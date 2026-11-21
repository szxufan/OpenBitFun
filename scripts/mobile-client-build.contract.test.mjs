import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
test('web protocol version matches the host account contract', () => {
 const host = readFileSync('src/crates/contracts/product-domains/src/account.rs','utf8').match(/CLIENT_PROTOCOL_VERSION: u32 = (\d+)/)[1];
 const web = readFileSync('src/shared/relay-transport/ClientBuild.ts','utf8').match(/CLIENT_PROTOCOL_VERSION = (\d+)/)[1];
 assert.equal(web,host);
});
