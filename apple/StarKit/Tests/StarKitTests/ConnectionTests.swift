import XCTest
@testable import StarKit
final class ConnectionTests:XCTestCase {
    func testCommandQuotesHostPathsAndArguments() throws {
        let profile=SSHProfile(host:"host",username:"user",remoteExecutable:"/apps/it’s screen/run.sh",movieDirectory:"/Movies/Bob's Movies;$(touch bad)")
        XCTAssertEqual(try profile.command(), "'/apps/it’s screen/run.sh' '--tv-serve' '--movies' '/Movies/Bob'\\''s Movies;$(touch bad)'")
    }
    func testBootstrapRejectsInvalidTokensAndURLs() throws {
        let valid=try JSONDecoder().decode(MovieSession.self,from:Data("{\"version\":1,\"port\":49152,\"token\":\"0123456789abcdef0123456789abcdef\"}".utf8))
        XCTAssertEqual(try valid.url(path:"/v1/catalog").host,"127.0.0.1")
        XCTAssertThrowsError(try valid.url(path:"/v1/../etc/passwd"))
        XCTAssertThrowsError(try JSONDecoder().decode(MovieSession.self,from:Data("{\"version\":2,\"port\":0,\"token\":\"bad\"}".utf8)).validate())
        XCTAssertThrowsError(try privateKey("invalid key"))
    }
}
