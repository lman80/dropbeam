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
        let rows = try decode(LossyArray<FriendLocations>.self, #"[{"friendId":"a","friendName":"A","online":true,"locations":[{"id":"n","name":"NAS","rights":{"upload":true},"freeBytes":5e9,"totalBytes":-1}],"status":"offline","checking":true,"checkedAt":1700000000000},{"friendId":"b","error":"x"}]"#).values
        precondition(rows[0].status == "offline" && rows[0].checking && rows[0].checkedAt == 1_700_000_000_000 && rows[0].locations[0].freeBytes == 5e9 && rows[0].locations[0].totalBytes == nil)
        precondition(rows[1].status == "error" && !rows[1].checking && rows[1].checkedAt == nil)
        let moving = try decode(LossyArray<Transfer>.self, #"[{"id":"t1","state":"transferring","locality":"internet","connDetail":{"path":"relay","rttMs":42.4,"upgrading":true},"verify":{"state":"done","checked":2,"total":2,"mismatched":[],"missing":["b"]},"integrity":[{"name":"a","verified":true},{"verified":true}]},{"id":"t2","locality":"local","connDetail":"bad","verify":7,"integrity":{}}]"#).values
        precondition(moving.count == 2 && moving[0].routeLabel == "Relay · going direct · 42 ms" && moving[0].verify?.missing == ["b"] && moving[0].integrity?.count == 1 && moving[0].integrityVerified)
        precondition(moving[1].routeLabel == "Local" && moving[1].connDetail == nil && moving[1].verify == nil && !moving[1].integrityVerified)
        let opened = try decode(OpenCodeResult.self, #"{"kind":"folderInvite","code":"dropbeam-folder:x"}"#)
        precondition(opened.kind == "folderInvite" && opened.code == "dropbeam-folder:x" && opened.name == nil)
        let folders = try decode(LossyArray<SharedFolder>.self, #"[{"id":"g","pairId":"a1","name":"Vacation","mode":"mirror","paused":true,"percent":250,"etaSeconds":null,"peerFiles":"x","members":[{"pairId":"a1","name":"Mong","canSetRole":true,"friendId":"f"},{"name":"no id"},{"pairId":"a2","pending":true,"name":""}],"summary":{"direction":"send","files":3,"bytes":1e400}},{"id":"missing pair"},{"id":"s","pairId":"s","state":"sending","queuedFiles":["a",1,"b"]}]"#).values
        precondition(folders.map(\.id) == ["g", "s"] && folders[0].paused && folders[0].percent == 100 && folders[0].etaSeconds == nil && folders[0].peerFiles == nil)
        precondition(folders[0].members.map(\.pairId) == ["a1", "a2"] && folders[0].members[1].name == "Waiting to join…" && folders[0].members[0].canSetRole)
        precondition(folders[0].summary?.files == 3 && folders[0].summary?.bytes == 0 && folders[1].busy && folders[1].queuedFiles == ["a", "b"] && folders[1].mirror)
        let verify = try decode(FolderVerify.self, #"{"peerOnline":true,"compared":true,"identical":false,"differences":2}"#)
        precondition(verify.differences == 2 && !verify.identical && verify.matched == 0)
        print("16 native model regression checks passed")
    }
}
