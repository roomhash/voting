import { createHash } from 'node:crypto'

const PIECE_LENGTH = 16 * 1024

function utf8(value) {
  return Buffer.from(String(value), 'utf8')
}

function bencode(value) {
  if (Buffer.isBuffer(value) || value instanceof Uint8Array) {
    const bytes = Buffer.from(value)
    return Buffer.concat([Buffer.from(`${bytes.length}:`), bytes])
  }
  if (typeof value === 'string') return bencode(utf8(value))
  if (Number.isSafeInteger(value)) return Buffer.from(`i${value}e`)
  if (Array.isArray(value)) {
    return Buffer.concat([Buffer.from('l'), ...value.map(bencode), Buffer.from('e')])
  }
  if (value && typeof value === 'object') {
    const entries = Object.entries(value).sort(([left], [right]) =>
      Buffer.compare(utf8(left), utf8(right))
    )
    return Buffer.concat([
      Buffer.from('d'),
      ...entries.flatMap(([key, item]) => [bencode(key), bencode(item)]),
      Buffer.from('e')
    ])
  }
  throw new TypeError('unsupported bencode value')
}

export function createSingleFileTorrent({
  contents,
  name,
  tracker,
  webSeed,
  exactSource
}) {
  const bytes = Buffer.from(contents)
  if (!bytes.length) throw new Error('torrent contents must not be empty')
  if (!name || !tracker || !webSeed || !exactSource) {
    throw new Error('torrent metadata is incomplete')
  }
  const pieceHashes = []
  for (let offset = 0; offset < bytes.length; offset += PIECE_LENGTH) {
    pieceHashes.push(createHash('sha1').update(bytes.subarray(offset, offset + PIECE_LENGTH)).digest())
  }
  const info = {
    length: bytes.length,
    name,
    'piece length': PIECE_LENGTH,
    pieces: Buffer.concat(pieceHashes),
    private: 0
  }
  const encodedInfo = bencode(info)
  const torrent = {
    announce: tracker,
    'announce-list': [[tracker]],
    'created by': 'RoomHash Roomlet build',
    encoding: 'UTF-8',
    info,
    'url-list': [webSeed],
    'x-roomhash-exact-source': exactSource
  }
  return {
    bytes: bencode(torrent),
    infoHash: createHash('sha1').update(encodedInfo).digest('hex')
  }
}
