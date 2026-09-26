// Generates the Worker's sealing keypair (X25519). Run once:
//   node genkey.js
// It prints (1) the JWK to store as the WORKER_SEAL_PRIV secret and (2) the
// public key (base64) that DropBeam bakes in (src-tauri/src/mailbox/push.rs,
// WORKER_SEAL_PUB). Keep the private JWK secret; rotating it means shipping a
// new app build with the new public key.
import { generateKeyPairSync } from 'node:crypto'

const { privateKey, publicKey } = generateKeyPairSync('x25519')
const jwk = privateKey.export({ format: 'jwk' })
const pub = Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url').toString('base64')
console.log('WORKER_SEAL_PRIV (secret, paste into `wrangler secret put WORKER_SEAL_PRIV`):')
console.log(JSON.stringify(jwk))
console.log('\nWORKER_SEAL_PUB (bake into the app):')
console.log(pub)
