import Foundation

// Run with swiftc Models.swift Formatters.swift tests/native-models.swift -o <test binary>.
@main struct NativeModelTests {
    static func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }
    static func main() throws {
        let friends = try decode(LossyArray<Friend>.self, #"[{"id":"alice","name":"Alice","avatar":42},{"name":"missing id"},null,{"id":"bob","name":"Bob"}]"#).values
        precondition(friends.map(\.id) == ["alice", "bob"] && friends[0].avatar == nil)
        let thread = try decode(ChatThread.self, #"{"friendId":"alice","messages":[{"id":"m1","peerId":"alice","fromMe":false,"ts":100,"text":7,"files":["a.heic",false]},{}]}"#)
        precondition(thread.messages.count == 1 && thread.messages[0].preview == "a.heic")
        let settings = try decode(Settings.self, #"{"theme":"dark","uploadLimitMbps":1e300,"playSounds":"bad","futureField":true}"#)
        precondition(settings.theme == "dark" && settings.uploadLimitMbps == nil && settings.playSounds == nil)
        let page = try decode(BrowserPage.self, #"{"entries":[],"hasMore":true,"cursor":"next"}"#)
        precondition(page.hasMore && page.cursor == "next")
        let emptyPage = try decode(BrowserPage.self, "{}")
        let rights = try decode(SharedLocation.self, #"{"id":"folder","rights":{"upload":"yes"}}"#).rights
        precondition(emptyPage.entries.isEmpty && !emptyPage.hasMore && !rights.upload && !rights.manage)
        let own = try decode(LossyArray<Friend>.self, #"[{"id":"mac","name":"Ashton laptop","ownDevice":true,"ownLabel":"Your Mac","deviceOs":"macos"},{"id":"f","name":"Mong","ownDevice":"bad"}]"#).values
        precondition(own[0].displayName == "Your Mac" && own[0].ownDevice && own[1].displayName == "Mong" && !own[1].ownDevice)
        let me = try decode(MyDevice.self, #"{"name":"Phone","accountPub":"abc","devices":[{"endpointId":"e1","name":"Phone","thisDevice":true},{"name":"no id"},{"endpointId":"e2","name":"Mac","deviceOs":"macos","lastSyncMs":1700000000000,"friendId":"mac"}]}"#)
        precondition(me.inAccount && me.devices.count == 2 && me.devices[0].thisDevice && me.devices[1].lastSyncMs == 1700000000000)
        print("7 native model regression checks passed")
    }
}
