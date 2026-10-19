import Foundation
import Network
import Crypto
import NIOCore
import NIOSSH
import NIOTransportServices

/// One authenticated SSH socket owns the command and all HTTP forwarding channels.
public actor SSHMovieConnection {
    private let group: NIOTSEventLoopGroup
    private let ssh: Channel
    private let command: Channel
    private let listener: Channel
    public let session: MovieSession
    private var closed=false
    private init(group:NIOTSEventLoopGroup, ssh:Channel, command:Channel, listener:Channel, session:MovieSession) { self.group=group; self.ssh=ssh; self.command=command; self.listener=listener; self.session=session }
    public static func connect(profile:SSHProfile, credential:SSHCredential) async throws -> SSHMovieConnection {
        let remote=try profile.command()
        let auth=try Authentication(profile:profile,credential:credential)
        let group=NIOTSEventLoopGroup(loopCount:1)
        var parent: Channel?; var service: Channel?; var server: Channel?
        do {
            let ready=Once<Void>(group.next())
            let tcp=NWProtocolTCP.Options(); tcp.enableKeepalive=true; tcp.keepaliveIdle=30; tcp.keepaliveInterval=15; tcp.keepaliveCount=3
            let channel=try await NIOTSConnectionBootstrap(group:group).tcpOptions(tcp).connectTimeout(.seconds(20)).channelInitializer { channel in
                channel.eventLoop.makeCompletedFuture {
                    try channel.pipeline.syncOperations.addHandlers(NIOSSHHandler(role:.client(.init(userAuthDelegate:auth, serverAuthDelegate:HostKey(expected:profile.hostKeyFingerprint))),allocator:channel.allocator,inboundChildChannelInitializer:nil), Ready(ready:ready))
                }
            }.connect(host:profile.host,port:profile.port).get()
            parent=channel
            let timeout=channel.eventLoop.scheduleTask(in:.seconds(20)) { ready.fail(SSHError.timeout); channel.close(promise:nil) }
            defer { timeout.cancel() }
            try await ready.future.get()
            try Task.checkCancellation()
            let bootstrap=Once<MovieSession>(channel.eventLoop)
            let command=try await channel.eventLoop.flatSubmit {
                channel.pipeline.handler(type:NIOSSHHandler.self).flatMap { handler in
                    let promise=channel.eventLoop.makePromise(of:Channel.self)
                    handler.createChannel(promise,channelType:.session) { child,_ in child.pipeline.addHandler(Bootstrap(command:remote,result:bootstrap)) }
                    return promise.futureResult
                }
            }.get()
            service=command
            let bootTimeout=channel.eventLoop.scheduleTask(in:.seconds(60)) { bootstrap.fail(SSHError.timeout); command.close(promise:nil) }
            var session:MovieSession
            do { session=try await bootstrap.future.get() } catch { bootTimeout.cancel(); throw error }
            bootTimeout.cancel(); try session.validate(); try Task.checkCancellation()
            let remotePort=session.port
            let localOrigin=try SocketAddress(ipAddress:"127.0.0.1",port:0)
            let limit=ForwardLimit()
            let listener=try await NIOTSListenerBootstrap(group:group).childChannelInitializer { inbound in
                guard limit.count < 16 else { return inbound.close() }
                limit.count += 1
                inbound.closeFuture.whenComplete { _ in limit.count -= 1 }
                return channel.eventLoop.flatSubmit {
                    channel.pipeline.handler(type:NIOSSHHandler.self).flatMap { handler in
                        let promise=channel.eventLoop.makePromise(of:Channel.self)
                        let target=SSHChannelType.DirectTCPIP(targetHost:"127.0.0.1",targetPort:remotePort,originatorAddress:inbound.remoteAddress ?? localOrigin)
                        handler.createChannel(promise,channelType:.directTCPIP(target)) { child,_ in
                            let pair=GlueHandler.matchedPair()
                            return child.pipeline.addHandlers(Wrapper(),pair.0).flatMap { inbound.pipeline.addHandler(pair.1) }
                        }
                        return promise.futureResult.map { _ in }.flatMapError { error in inbound.close().flatMapThrowing { throw error } }
                    }
                }
            }.bind(host:"127.0.0.1",port:0).get()
            server=listener
            guard let port=listener.localAddress?.port else { throw SSHError.disconnected }
            session.port=port
            try Task.checkCancellation()
            return SSHMovieConnection(group:group,ssh:channel,command:command,listener:listener,session:session)
        } catch {
            try? await server?.close().get(); try? await service?.close().get(); try? await parent?.close().get(); try? await group.shutdownGracefully()
            throw error
        }
    }
    public func isConnected() -> Bool { !closed && ssh.isActive && command.isActive }
    public func close() async {
        guard !closed else { return }; closed=true
        try? await listener.close().get(); try? await command.close().get(); try? await ssh.close().get(); try? await group.shutdownGracefully()
    }
}
private final class ForwardLimit: @unchecked Sendable { var count=0 } // one event loop
private final class Once<T>: @unchecked Sendable {
    private var promise:EventLoopPromise<T>?
    let future:EventLoopFuture<T>
    init(_ loop:EventLoop) { let p=loop.makePromise(of:T.self); promise=p; future=p.futureResult }
    func succeed(_ value:T) { let current=promise; promise=nil; current?.succeed(value) }
    func fail(_ error:Error) { let current=promise; promise=nil; current?.fail(error) }
}
private final class Authentication: NIOSSHClientUserAuthenticationDelegate, @unchecked Sendable {
    let profile:SSHProfile; let credential:SSHCredential; let key:NIOSSHPrivateKey?; var offered=false
    init(profile:SSHProfile,credential:SSHCredential) throws { self.profile=profile; self.credential=credential; if case .privateKey(let text)=credential { key=try privateKey(text) } else { key=nil } }
    func nextAuthenticationType(availableMethods:NIOSSHAvailableUserAuthenticationMethods,nextChallengePromise:EventLoopPromise<NIOSSHUserAuthenticationOffer?>) {
        guard !offered else { nextChallengePromise.succeed(nil); return }; offered=true
        if let key=key, availableMethods.contains(.publicKey) { nextChallengePromise.succeed(.init(username:profile.username,serviceName:"",offer:.privateKey(.init(privateKey:key)))) }
        else if case .password(let password)=credential, availableMethods.contains(.password) { nextChallengePromise.succeed(.init(username:profile.username,serviceName:"",offer:.password(.init(password:password)))) }
        else { nextChallengePromise.fail(SSHError.authentication) }
    }
}
private final class HostKey: NIOSSHClientServerAuthenticationDelegate {
    let expected:String
    init(expected:String) { self.expected=expected }
    func validateHostKey(hostKey:NIOSSHPublicKey,validationCompletePromise:EventLoopPromise<Void>) {
        let pieces=String(openSSHPublicKey:hostKey).split(separator:" ")
        guard pieces.count>=2, let bytes=Data(base64Encoded:String(pieces[1])) else { validationCompletePromise.fail(SSHError.invalidBootstrap); return }
        let fingerprint="SHA256:"+Data(SHA256.hash(data:bytes)).base64EncodedString().replacingOccurrences(of:"=",with:"")
        if expected == fingerprint { validationCompletePromise.succeed(()) }
        else { validationCompletePromise.fail(expected.isEmpty ? SSHError.hostKeyRequired(fingerprint) : SSHError.hostKeyChanged(fingerprint)) }
    }
}
private final class Ready: ChannelInboundHandler {
    typealias InboundIn=ByteBuffer
    let ready:Once<Void>
    init(ready:Once<Void>) { self.ready=ready }
    func userInboundEventTriggered(context:ChannelHandlerContext,event:Any) { if event is UserAuthSuccessEvent { ready.succeed(()) }; context.fireUserInboundEventTriggered(event) }
    func errorCaught(context:ChannelHandlerContext,error:Error) { ready.fail(error); context.close(promise:nil) }
    func channelInactive(context:ChannelHandlerContext) { ready.fail(SSHError.disconnected); context.fireChannelInactive() }
}
private final class Bootstrap: ChannelInboundHandler {
    typealias InboundIn=SSHChannelData
    let command:String; let result:Once<MovieSession>; var bytes=[UInt8](); var complete=false
    init(command:String,result:Once<MovieSession>) { self.command=command; self.result=result }
    func channelActive(context:ChannelHandlerContext) {
        context.triggerUserOutboundEvent(SSHChannelRequestEvent.ExecRequest(command:command,wantReply:true)).whenFailure { error in self.result.fail(error); context.close(promise:nil) }
    }
    func channelRead(context:ChannelHandlerContext,data:NIOAny) {
        guard !complete else { return }
        let packet=unwrapInboundIn(data)
        guard packet.type == .channel, case .byteBuffer(let buffer)=packet.data else { return }
        bytes.append(contentsOf:buffer.readableBytesView)
        guard bytes.count<=16384 else { result.fail(SSHError.invalidBootstrap); context.close(promise:nil); return }
        if let end=bytes.firstIndex(of:10) {
            do { let session=try JSONDecoder().decode(MovieSession.self,from:Data(bytes[..<end])); try session.validate(); complete=true; bytes=[]; result.succeed(session) }
            catch { result.fail(error); context.close(promise:nil) }
        }
    }
    func errorCaught(context:ChannelHandlerContext,error:Error) { result.fail(error); context.close(promise:nil) }
    func channelInactive(context:ChannelHandlerContext) { result.fail(SSHError.disconnected); context.fireChannelInactive() }
}
private final class Wrapper: ChannelDuplexHandler {
    typealias InboundIn=SSHChannelData; typealias InboundOut=ByteBuffer; typealias OutboundIn=ByteBuffer; typealias OutboundOut=SSHChannelData
    func channelRead(context:ChannelHandlerContext,data:NIOAny) { let packet=unwrapInboundIn(data); if case .byteBuffer(let buffer)=packet.data { context.fireChannelRead(wrapInboundOut(buffer)) } }
    func write(context:ChannelHandlerContext,data:NIOAny,promise:EventLoopPromise<Void>?) { context.write(wrapOutboundOut(.init(type:.channel,data:.byteBuffer(unwrapOutboundIn(data)))),promise:promise) }
}
