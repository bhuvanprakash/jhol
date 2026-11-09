// async/await lowered onto the engine's own generator machinery: the body runs
// synchronously to the first await, then each suspension is resumed from the
// settled value on the microtask queue.
async function seq() {
  let t = 0;
  for (let i = 0; i < 3; i++) t += await Promise.resolve(i);
  return t;
}
const arrow = async (x) => (await x) * 2;
class K {
  async m() {
    return await Promise.resolve("m");
  }
}
(async () => {
  console.log("seq", await seq());
  console.log("arrow", await arrow(Promise.resolve(21)));
  console.log("method", await new K().m());
  console.log("plain", await 7);
  try {
    await null.x;
  } catch (e) {
    console.log("type", e.name);
  }
  const vals = await Promise.all([seq(), arrow(1)]);
  console.log("all", vals.join(","));
})();
