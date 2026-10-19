import Foundation
import StarKit

@main struct Probe {
    static func main() async throws {
        let args=CommandLine.arguments
        guard args.count==7 else { print("starkit-probe HOST PORT USER FINGERPRINT REMOTE_EXECUTABLE MOVIE_DIRECTORY; secret via STAR_SSH_PASSWORD or STAR_SSH_PRIVATE_KEY"); return }
        let profile=SSHProfile(host:args[1],port:Int(args[2]) ?? 22,username:args[3],hostKeyFingerprint:args[4],remoteExecutable:args[5],movieDirectory:args[6])
        let env=ProcessInfo.processInfo.environment
        let credential:SSHCredential=env["STAR_SSH_PRIVATE_KEY"].map { .privateKey($0) } ?? .password(env["STAR_SSH_PASSWORD"] ?? "")
        let connection=try await SSHMovieConnection.connect(profile:profile,credential:credential)
        do {
            let session=await connection.session
            let (data,response)=try await URLSession.shared.data(from:session.url(path:"/v1/catalog"))
            guard (response as? HTTPURLResponse)?.statusCode==200 else { throw SSHError.disconnected }
            print("Catalog received: \(data.count) bytes through one native SSH connection")
            if let object=try JSONSerialization.jsonObject(with:data) as? [String:Any], let movies=object["movies"] as? [[String:Any]], let path=movies.first?["media_path"] as? String {
                var request=URLRequest(url:try session.url(path:path)); request.setValue("bytes=0-4095",forHTTPHeaderField:"Range")
                let (bytes,reply)=try await URLSession.shared.data(for:request)
                guard (reply as? HTTPURLResponse)?.statusCode == 206, bytes.count == 4096 else { throw SSHError.disconnected }
                if let source=env["STAR_MEDIA_SOURCE"] { guard bytes == (try Data(contentsOf:URL(fileURLWithPath:source))).prefix(4096) else { throw SSHError.invalidBootstrap } }
                print("Original media range received: 4096 unchanged bytes through the same SSH connection")
            }
            if let output=env["STAR_CATALOG_OUTPUT"] { try data.write(to:URL(fileURLWithPath:output)) }
            await connection.close()
        } catch { await connection.close(); throw error }
    }
}
