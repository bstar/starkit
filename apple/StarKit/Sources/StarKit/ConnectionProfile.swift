import Foundation
import Crypto
import NIOSSH

/// Shared connection settings. Secrets belong in the client Keychain.
public struct SSHProfile: Codable, Equatable, Sendable {
    public var host: String
    public var port: Int
    public var username: String
    public var hostKeyFingerprint: String
    public var remoteExecutable: String
    public var movieDirectory: String
    public var projectDirectory: String?
    public init(host: String, port: Int = 22, username: String, hostKeyFingerprint: String = "", remoteExecutable: String, movieDirectory: String, projectDirectory: String? = nil) {
        self.host=host; self.port=port; self.username=username; self.hostKeyFingerprint=hostKeyFingerprint; self.remoteExecutable=remoteExecutable; self.movieDirectory=movieDirectory; self.projectDirectory=projectDirectory
    }
    public func command() throws -> String {
        guard !host.isEmpty, !host.hasPrefix("-"), !username.isEmpty, (1...65535).contains(port), !remoteExecutable.isEmpty, !movieDirectory.isEmpty,
              [host,username,remoteExecutable,movieDirectory].allSatisfy({ !$0.contains("\0") && !$0.contains("\n") && !$0.contains("\r") }) else { throw SSHError.invalidProfile }
        var args=[remoteExecutable,"--tv-serve","--movies",movieDirectory]
        if let project=projectDirectory, !project.isEmpty {
            guard !project.contains("\0"), !project.contains("\n"), !project.contains("\r") else { throw SSHError.invalidProfile }
            args += ["--project",project]
        }
        return args.map(Self.quote).joined(separator:" ")
    }
    public static func quote(_ value: String) -> String { "'" + value.replacingOccurrences(of:"'",with:"'\\''") + "'" }
}
public enum SSHCredential: Sendable { case password(String), privateKey(String) }
public enum SSHError: Error, LocalizedError, Equatable {
    case invalidProfile, hostKeyRequired(String), hostKeyChanged(String), authentication, timeout, disconnected, invalidBootstrap, unsupportedKey
    public var errorDescription: String? {
        switch self {
        case .invalidProfile: return "Enter the SSH host, account, movie folder and server executable."
        case .hostKeyRequired(let key): return "Confirm this server's SSH fingerprint: \(key)"
        case .hostKeyChanged(let key): return "The server SSH key has changed: \(key). Check the connection settings."
        case .authentication: return "SSH sign-in failed. Check the account and password or private key."
        case .timeout: return "The SSH movie service did not respond in time."
        case .disconnected: return "The SSH movie connection closed."
        case .invalidBootstrap: return "The host did not return a compatible STAR/KIT movie session."
        case .unsupportedKey: return "Use an unencrypted OpenSSH Ed25519 key, a P-256 PEM key, or password authentication."
        }
    }
}
public struct MovieSession: Codable, Equatable, Sendable {
    public let version: Int
    public var port: Int
    public let token: String
    public func validate() throws {
        guard version == 1, (1...65535).contains(port), token.count == 32, token.utf8.allSatisfy({ (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) }) else { throw SSHError.invalidBootstrap }
    }
    public func url(path: String) throws -> URL {
        try validate()
        guard path.hasPrefix("/v1/"), !path.contains(".."), !path.contains("?"), !path.contains("#") else { throw SSHError.invalidBootstrap }
        var parts=URLComponents(); parts.scheme="http"; parts.host="127.0.0.1"; parts.port=port; parts.path=path; parts.queryItems=[URLQueryItem(name:"token",value:token)]
        guard let url=parts.url else { throw SSHError.invalidBootstrap }; return url
    }
}
func privateKey(_ text: String) throws -> NIOSSHPrivateKey {
    if let key=try? P256.Signing.PrivateKey(pemRepresentation:text) { return NIOSSHPrivateKey(p256Key:key) }
    let body=text.components(separatedBy:.newlines).filter { !$0.hasPrefix("-----") }.joined()
    guard let bytes=Data(base64Encoded:body), bytes.count <= 16384 else { throw SSHError.unsupportedKey }
    var reader=KeyReader(data:bytes)
    guard try reader.take(15) == Data("openssh-key-v1\0".utf8), try reader.string() == Data("none".utf8), try reader.string() == Data("none".utf8), try reader.string().isEmpty, try reader.integer() == 1 else { throw SSHError.unsupportedKey }
    _ = try reader.string()
    var secret=KeyReader(data:try reader.string())
    guard try secret.integer() == secret.integer(), try secret.string() == Data("ssh-ed25519".utf8) else { throw SSHError.unsupportedKey }
    let publicBytes=try secret.string(); let privateBytes=try secret.string()
    guard publicBytes.count == 32, privateBytes.count == 64, privateBytes.suffix(32) == publicBytes else { throw SSHError.unsupportedKey }
    let key=try Curve25519.Signing.PrivateKey(rawRepresentation:privateBytes.prefix(32))
    guard key.publicKey.rawRepresentation == publicBytes else { throw SSHError.unsupportedKey }
    return NIOSSHPrivateKey(ed25519Key:key)
}
private struct KeyReader {
    let data: Data; var cursor=0
    mutating func take(_ n:Int) throws -> Data { guard n>=0, n<=data.count-cursor else { throw SSHError.unsupportedKey }; defer { cursor+=n }; return data.subdata(in:cursor..<cursor+n) }
    mutating func integer() throws -> Int { try take(4).reduce(0) { ($0 << 8) | Int($1) } }
    mutating func string() throws -> Data { let size=try integer(); return try take(size) }
}
