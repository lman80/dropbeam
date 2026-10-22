import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { AUTO_ZOOMS, hardwareZoom, MAX_DIGITAL_ZOOM, zoomCrop, zoomsForTick } from '../src/lib/qrZoom.ts'

test('a zoomed read takes the centre of the frame at full resolution and enlarges it (#85)', () => {
  // The whole 1080p frame is read at 960 wide (as before).
  assert.deepEqual(zoomCrop(1920, 1080, 1), { sx: 0, sy: 0, sw: 1920, sh: 1080, dw: 960, dh: 540 })
  // ×2: the middle quarter, pixel for pixel — a far-away code keeps its detail.
  assert.deepEqual(zoomCrop(1920, 1080, 2), { sx: 480, sy: 270, sw: 960, sh: 540, dw: 960, dh: 540 })
  // ×3 on a 720p webcam: the small centre is enlarged ×2 so each square spans enough pixels.
  assert.deepEqual(zoomCrop(1280, 720, 3), { sx: 427, sy: 240, sw: 427, sh: 240, dw: 854, dh: 480 })
  // Nonsense zooms read the whole frame; a crop always stays inside it.
  for (const z of [0, -2, Number.NaN, 0.5]) assert.equal(zoomCrop(1280, 720, z).sw, 1280)
  for (const z of [1, 1.5, 2, 3, MAX_DIGITAL_ZOOM, 8]) {
    const r = zoomCrop(640, 480, z)
    assert.ok(r.sx >= 0 && r.sy >= 0 && r.sx + r.sw <= 640 && r.sy + r.sh <= 480 && r.dw <= 960 && r.dw >= r.sw)
  }
})

test('without a chosen zoom every frame reads the whole picture plus one closer view in turn', () => {
  const seen = new Set<number>()
  for (let tick = 0; tick < 6; tick++) {
    const zs = zoomsForTick(tick)
    assert.equal(zs.length, 2, 'two reads a frame, not more')
    assert.equal(zs[0], 1)
    seen.add(zs[1])
  }
  assert.deepEqual([...seen].sort(), [...AUTO_ZOOMS].sort(), 'every auto zoom is tried within a few frames')
  // A chosen zoom is always read, sometimes with a still closer look.
  for (let tick = 0; tick < 6; tick++) assert.equal(zoomsForTick(tick, 2)[0], 2)
  assert.ok(zoomsForTick(1, 2).length === 2 && zoomsForTick(1, 2)[1] > 2)
  assert.deepEqual(zoomsForTick(0, 2), [2])
})

test('the camera’s own zoom is used only when it really has a range', () => {
  const track = (zoom?: object) => ({ getCapabilities: () => (zoom ? { zoom } : {}) }) as unknown as MediaStreamTrack
  assert.deepEqual(hardwareZoom(track({ min: 1, max: 5, step: 0.5 })), { min: 1, max: 5, step: 0.5 })
  assert.deepEqual(hardwareZoom(track({ min: 100, max: 400 })), { min: 100, max: 400, step: 0.1 })
  assert.equal(hardwareZoom(track({ min: 1, max: 1 })), null)
  assert.equal(hardwareZoom(track()), null)
  assert.equal(hardwareZoom(undefined), null)
  assert.equal(hardwareZoom({} as MediaStreamTrack), null, 'WebKit: no getCapabilities')
})
