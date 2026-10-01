const {test} = require('node:test');
const assert = require('node:assert/strict');
const {capture} = require('./capture.cjs');
test('transient compositor loss recovers without changing capture arguments', async () => {
  let attempts = 0;
  const delays = [];
  const image = {};
  assert.equal(await capture({capturePage: async (rect, options) => {
    assert.equal(rect, undefined);
    assert.deepEqual(options, {stayHidden:true, stayAwake:true});
    if (++attempts < 3) throw Error('UnknownVizError');
    return image;
  }}, async ms => delays.push(ms)), image);
  assert.deepEqual(delays, [25, 50]);
});
test('persistent compositor loss is bounded', async () => {
  let attempts = 0;
  await assert.rejects(capture({capturePage: async () => {
    attempts++; throw Error('UnknownVizError');
  }}, async () => {}), /UnknownVizError/);
  assert.equal(attempts, 5);
});
test('unrelated failures are not retried', async () => {
  let attempts = 0;
  await assert.rejects(capture({capturePage: async () => {
    attempts++; throw Error('Renderer destroyed');
  }}, async () => assert.fail('Unexpected retry')), /Renderer destroyed/);
  assert.equal(attempts, 1);
});
