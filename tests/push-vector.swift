// swift tests/push-vector.swift — the Notification Service Extension's decrypt,
// checked against the Rust-generated vector in docs/mailbox-vectors.json.
import CryptoKit
import Foundation

let url = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "docs/mailbox-vectors.json")
let v = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
let p = v["push_preview"] as! [String: String]
let me = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: Data(base64Encoded: p["secret"]!)!)
let raw = Data(base64Encoded: p["sealed"]!)!
let eph = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: raw.prefix(32))
let shared = try me.sharedSecretFromKeyAgreement(with: eph)
let key = shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: raw.prefix(32) + me.publicKey.rawRepresentation,
                                         sharedInfo: Data("dropbeam-push-v1".utf8), outputByteCount: 32)
let plain = try ChaChaPoly.open(ChaChaPoly.SealedBox(combined: raw.dropFirst(32)), using: key)
guard String(decoding: plain, as: UTF8.self) == p["plaintext"] else { print("MISMATCH"); exit(1) }
print("push preview vector OK")
