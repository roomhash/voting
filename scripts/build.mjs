import { chmod, copyFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { createSingleFileTorrent } from './torrent.mjs'

const root = new URL('../', import.meta.url)
const dist = new URL('../dist/', import.meta.url)
const entry = 'voting.wasm'
const torrentFile = 'voting.torrent'
const rawBase = 'https://raw.githubusercontent.com/roomhash/voting/main/dist/'
const webSeed = `${rawBase}${entry}`
const exactSource = `${rawBase}${torrentFile}`
const tracker = 'wss://tracker.openwebtorrent.com'
execFileSync('cargo', ['build', '--release', '--target', 'wasm32-unknown-unknown'], { cwd: root, stdio: 'inherit' })
await rm(dist, { recursive: true, force: true })
await mkdir(dist, { recursive: true })

const wasmSource = new URL('../target/wasm32-unknown-unknown/release/voting.wasm', import.meta.url)
const wasmTarget = new URL(entry, dist)
await copyFile(wasmSource, wasmTarget)
await chmod(wasmTarget, 0o644)
const wasm = await readFile(wasmTarget)
const torrent = createSingleFileTorrent({
  contents: wasm,
  name: entry,
  tracker,
  webSeed,
  exactSource
})
await writeFile(new URL(torrentFile, dist), torrent.bytes)
const magnet = new URL('magnet:')
magnet.searchParams.set('xt', `urn:btih:${torrent.infoHash}`)
magnet.searchParams.set('dn', entry)
magnet.searchParams.set('tr', tracker)
magnet.searchParams.set('ws', webSeed)
magnet.searchParams.set('xs', exactSource)
const manifest = {
  schema: 'roomhash.app/v1',
  id: 'org.roomhash.voting',
  name: 'Shared Polls',
  description: 'Browse and create auditable public polls with creator deletion, hash-deduplicated ballots, and automatic expiry within 14 days.',
  i18n: {
    en: {
      name: 'Shared Polls',
      description: 'Browse and create auditable public polls with creator deletion, hash-deduplicated ballots, and automatic expiry within 14 days.',
      notice: 'Results are local collection views, not globally consistent totals. Different peers may show different counts until their event sets converge.'
    },
    'zh-CN': {
      name: '共享投票',
      description: '浏览或创建公开投票，支持创建者删除、按用户 Hash 排重，并在最长 14 天后自动清理。',
      notice: '结果是当前节点的本地收集视图，并非全局强一致统计；在事件集合收敛前，不同参与者看到的票数可能不同。'
    }
  },
  version: '1.1.1',
  runtime: 'wasm',
  abi: 'portable-surface-v1',
  entry,
  sha256: createHash('sha256').update(wasm).digest('hex'),
  permissions: ['channel.messages', 'storage:512kb'],
  notice: 'Results are local collection views, not globally consistent totals. Different peers may show different counts until their event sets converge.',
  distribution: {
    torrent: torrentFile,
    infoHash: torrent.infoHash,
    entrySize: wasm.length,
    webSeed,
    exactSource,
    magnet: magnet.href
  }
}
await writeFile(new URL('roomhash.json', dist), `${JSON.stringify(manifest, null, 2)}\n`)
await copyFile(new URL('../README.md', import.meta.url), new URL('README.md', dist))
await copyFile(new URL('../LICENSE', import.meta.url), new URL('LICENSE', dist))
console.log(`Built ${entry}: ${wasm.length} bytes, sha256 ${manifest.sha256}, infoHash ${torrent.infoHash}`)
