import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, describe, it } from 'node:test'

import { list, restore, restoreSync, trash, trashSync } from '../index.js'

const scratch = mkdtempSync(join(tmpdir(), 'trash-pro-node-test-'))
const fallback = join(scratch, 'fallback')

after(() => rmSync(scratch, { recursive: true, force: true }))

function makeFile(name, contents = 'x') {
  const path = join(scratch, name)
  mkdirSync(join(path, '..'), { recursive: true })
  writeFileSync(path, contents)
  return path
}

describe('trashSync', () => {
  it('moves items to the OS trash (or the temp fallback where none exists)', () => {
    const file = makeFile('sync-basic.txt', 'basic')
    const results = trashSync([file])

    assert.equal(results.length, 1)
    assert.equal(results[0].path, file)
    assert.equal(results[0].ok, true)
    assert.ok(['trash', 'temp'].includes(results[0].method))
    assert.equal(existsSync(file), false)
  })

  it('moves a directory tree with forceTemp into the fallback dir', () => {
    makeFile('tree/inner/deep.txt', 'deep')
    makeFile('tree/top.txt', 'top')
    const dir = join(scratch, 'tree')
    const destRoot = join(scratch, 'dest')

    const results = trashSync([dir], { forceTemp: true, tempDir: destRoot })

    assert.equal(results[0].ok, true)
    assert.equal(results[0].method, 'temp')
    assert.equal(results[0].destination, join(destRoot, 'tree'))
    assert.equal(existsSync(dir), false)
    assert.equal(readFileSync(join(destRoot, 'tree', 'inner', 'deep.txt'), 'utf8'), 'deep')
    assert.equal(readFileSync(join(destRoot, 'tree', 'top.txt'), 'utf8'), 'top')
  })

  it('de-duplicates name collisions in the fallback dir', () => {
    const a = makeFile('collide-a/same.txt', 'A')
    const b = makeFile('collide-b/same.txt', 'B')
    const destRoot = join(scratch, 'dest-collide')

    const [ra, rb] = trashSync([a, b], { forceTemp: true, tempDir: destRoot })

    assert.equal(ra.destination, join(destRoot, 'same.txt'))
    assert.equal(rb.destination, join(destRoot, 'same.txt.1'))
    assert.equal(readFileSync(ra.destination, 'utf8'), 'A')
    assert.equal(readFileSync(rb.destination, 'utf8'), 'B')
  })

  it('reports per-item errors without throwing and keeps processing the batch', () => {
    const good = makeFile('mixed-good.txt', 'good')
    const missing = join(scratch, 'mixed-missing.txt')

    const results = trashSync([missing, good], { forceTemp: true, tempDir: join(scratch, 'dest-mixed') })

    assert.equal(results.length, 2)
    assert.equal(results[0].ok, false)
    assert.equal(results[0].path, missing)
    assert.match(results[0].error, /invalid target/)
    assert.equal(results[1].ok, true)
    assert.equal(existsSync(good), false)
  })

  it('rejects root targets as hard errors', () => {
    const [result] = trashSync(['/'], { forceTemp: true, tempDir: join(scratch, 'dest-root') })
    assert.equal(result.ok, false)
    assert.match(result.error, /invalid target/)
  })
})

describe('trash (async)', () => {
  it('resolves per-item results on a worker thread', async () => {
    const file = makeFile('async-basic.txt', 'async')
    const results = await trash([file], { forceTemp: true, tempDir: join(scratch, 'dest-async') })

    assert.equal(results.length, 1)
    assert.equal(results[0].ok, true)
    assert.equal(results[0].method, 'temp')
    assert.equal(existsSync(file), false)
  })
})

describe('edge cases', () => {
  it('returns an empty array for an empty batch', () => {
    assert.deepEqual(trashSync([]), [])
  })

  it('handles unicode and spaced names', () => {
    const file = makeFile('空 目录/文件 名.txt', '内容')
    const destRoot = join(scratch, 'dest-unicode')

    const [result] = trashSync([file], { forceTemp: true, tempDir: destRoot })

    assert.equal(result.ok, true)
    assert.equal(result.destination, join(destRoot, '文件 名.txt'))
    assert.equal(readFileSync(result.destination, 'utf8'), '内容')
  })

  it('exposes named exports to ESM importers', async () => {
    const mod = await import('../index.js')
    assert.equal(typeof mod.trash, 'function')
    assert.equal(typeof mod.trashSync, 'function')
    assert.equal(mod.trashSync, trashSync)
  })
})

describe('recovery', () => {
  it('lists fallback items with their original paths', () => {
    const src = makeFile('recover-src/remember.txt', 'R')
    const destRoot = join(scratch, 'dest-list')

    trashSync([src], { forceTemp: true, tempDir: destRoot })

    const items = list(destRoot)
    assert.equal(items.length, 1)
    assert.equal(items[0].item, join(destRoot, 'remember.txt'))
    assert.equal(items[0].originalPath, src)
    assert.equal(items[0].kind, 'file')
    assert.ok(items[0].deletedAt.length > 0)
  })

  it('restores items back to their original paths (async)', async () => {
    const src = makeFile('recover-back.txt', 'payload')
    const destRoot = join(scratch, 'dest-restore')

    const [moved] = trashSync([src], { forceTemp: true, tempDir: destRoot })
    assert.equal(existsSync(src), false)

    const [result] = await restore([moved.destination])
    assert.equal(result.ok, true)
    assert.equal(result.restoredTo, src)
    assert.equal(readFileSync(src, 'utf8'), 'payload')
    assert.equal(existsSync(moved.destination), false)
    assert.equal(list(destRoot).length, 0)
  })

  it('restore reports conflicts and honors overwrite (sync)', () => {
    const src = makeFile('recover-conflict.txt', 'original')
    const destRoot = join(scratch, 'dest-conflict')

    const [moved] = trashSync([src], { forceTemp: true, tempDir: destRoot })
    writeFileSync(src, 'new occupant')

    const [conflict] = restoreSync([moved.destination])
    assert.equal(conflict.ok, false)
    assert.match(conflict.error, /occupied/)
    assert.equal(readFileSync(src, 'utf8'), 'new occupant')

    const [overwritten] = restoreSync([moved.destination], { overwrite: true })
    assert.equal(overwritten.ok, true)
    assert.equal(readFileSync(src, 'utf8'), 'original')
  })

  it('rejects items without a provenance record', () => {
    const destRoot = join(scratch, 'dest-unmanaged')
    mkdirSync(destRoot, { recursive: true })
    const foreign = join(destRoot, 'not-ours.txt')
    writeFileSync(foreign, '?')

    const [result] = restoreSync([foreign])
    assert.equal(result.ok, false)
    assert.match(result.error, /no trash-pro provenance record/)
  })
})
