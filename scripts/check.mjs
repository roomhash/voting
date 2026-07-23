import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFile, readdir } from 'node:fs/promises'
import { PortableWasmRuntime, validateScene } from './portable-test-host.mjs'
import { createSingleFileTorrent } from './torrent.mjs'

const dist = new URL('../dist/', import.meta.url)
const bytes = await readFile(new URL('voting.wasm', dist))
const torrentBytes = await readFile(new URL('voting.torrent', dist))
const manifest = JSON.parse(await readFile(new URL('roomhash.json', dist), 'utf8'))
const module = await WebAssembly.compile(bytes)

assert.deepEqual(WebAssembly.Module.imports(module), [], 'Portable applications must have no imports')

const context = (seed, nickname) => ({
  nickname, peerId: '', identitySeed: seed.repeat(64), channelId: 'channel-check',
  instanceId: 'vote-check', locale: 'zh-CN', theme: 'dark', savedState: null,
  nowMs: 1_800_000_000_000
})

async function runtime(seed = 'a', nickname = 'Alice') {
  const current = new PortableWasmRuntime()
  const output = await current.load(bytes, context(seed, nickname))
  validateScene(output.scene)
  return { current, output }
}

const { current, output } = await runtime()
assert.equal(output.scene.width, 960)
assert(JSON.stringify(output.scene).includes('共享投票'), 'Voting WASM title is stale')
assert.equal(output.scene.height, 640)

for (const [width, height] of [[320, 480], [375, 812], [768, 1024], [1440, 900]]) {
  const resized = current.dispatch({ kind: 'viewport', width, height, dpr: 2, fullscreen: width >= 1000 })
  validateScene(resized.scene)
  assert.equal(resized.scene.width, width)
  assert.equal(resized.scene.height, height)
  assert(!JSON.stringify(resized).includes('roomhash-form'), 'Voting leaked the HTML form ABI')
}

current.dispatch({ kind: 'viewport', width: 375, height: 812, dpr: 3, fullscreen: false })
const titleEffect = current.dispatch({ kind: 'pointer', phase: 'down', pointerId: 1, x: 40, y: 260, buttons: 1, pressure: 0.5 })
assert(titleEffect.effects?.some((effect) => effect.type === 'text-input' && effect.requestId === 'poll-title'))
current.dispatch({ kind: 'text', requestId: 'poll-title', value: '周五午餐', selectionStart: 4, selectionEnd: 4 })
current.dispatch({ kind: 'text', requestId: 'poll-options', value: '面条\n米饭\n沙拉', selectionStart: 8, selectionEnd: 8 })
const created = current.dispatch({ kind: 'pointer', phase: 'down', pointerId: 2, x: 180, y: 594, buttons: 1, pressure: 0.5 })
assert.equal(created.events?.[0]?.type, 'poll-created')
validateScene(created.scene)

const selected = current.dispatch({ kind: 'pointer', phase: 'down', pointerId: 3, x: 60, y: 430, buttons: 1, pressure: 0.5 })
assert.equal(selected.events?.length || 0, 0)
const voted = current.dispatch({ kind: 'pointer', phase: 'down', pointerId: 4, x: 180, y: 765, buttons: 1, pressure: 0.5 })
assert.equal(voted.events?.[0]?.type, 'ballot')

const snapshot = current.dispatch({ kind: 'state-request' }).snapshot
assert.equal(snapshot.events.length, 2)
const { current: peer } = await runtime('b', 'Bob')
for (const event of snapshot.events.toReversed()) peer.dispatch({ kind: 'remote', event })
assert.deepEqual(peer.dispatch({ kind: 'state-request' }).snapshot.events, snapshot.events)

const fullscreen = current.dispatch({ kind: 'pointer', phase: 'down', pointerId: 5, x: 320, y: 30, buttons: 1, pressure: 0.5 })
assert(fullscreen.effects?.some((effect) => effect.type === 'fullscreen'), 'Fullscreen must be requested by the WASM UI')

assert.equal(manifest.schema, 'roomhash.app/v1')
assert.equal(manifest.id, 'org.roomhash.voting')
assert.equal(manifest.runtime, 'wasm')
assert.equal(manifest.abi, 'portable-surface-v1')
assert.equal(manifest.entry, 'voting.wasm')
assert.equal(manifest.sha256, createHash('sha256').update(bytes).digest('hex'))
const expectedTorrent = createSingleFileTorrent({
  contents: bytes,
  name: 'voting.wasm',
  tracker: 'wss://tracker.openwebtorrent.com',
  webSeed: 'https://raw.githubusercontent.com/roomhash/voting/main/dist/voting.wasm',
  exactSource: 'https://raw.githubusercontent.com/roomhash/voting/main/dist/voting.torrent'
})
assert.deepEqual(torrentBytes, expectedTorrent.bytes)
assert.equal(manifest.distribution?.torrent, 'voting.torrent')
assert.equal(manifest.distribution?.infoHash, expectedTorrent.infoHash)
assert.equal(manifest.distribution?.entrySize, bytes.length)
assert.equal(manifest.distribution?.webSeed, 'https://raw.githubusercontent.com/roomhash/voting/main/dist/voting.wasm')
assert.equal(manifest.distribution?.exactSource, 'https://raw.githubusercontent.com/roomhash/voting/main/dist/voting.torrent')
const magnet = new URL(manifest.distribution?.magnet)
assert.equal(magnet.protocol, 'magnet:')
assert.equal(magnet.searchParams.get('xt'), `urn:btih:${expectedTorrent.infoHash}`)
assert.equal(magnet.searchParams.get('ws'), manifest.distribution.webSeed)
assert.equal(magnet.searchParams.get('xs'), manifest.distribution.exactSource)
assert(bytes.length <= 10 * 1024 * 1024)
assert.deepEqual((await readdir(dist)).sort(), ['LICENSE', 'README.md', 'roomhash.json', 'voting.torrent', 'voting.wasm'])

console.log(`Portable Voting OK: responsive 320x480–1440x900, ${bytes.length} bytes, sha256 ${manifest.sha256}, infoHash ${expectedTorrent.infoHash}`)
