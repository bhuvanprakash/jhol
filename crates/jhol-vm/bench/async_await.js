async function seq(n) {
  let t = 0;
  for (let i = 0; i < n; i++) t += await Promise.resolve(1);
  return t;
}
async function par(n) {
  const ps = [];
  for (let i = 0; i < n; i++) ps.push(Promise.resolve(i));
  const vs = await Promise.all(ps);
  let t = 0;
  for (let i = 0; i < n; i++) t += vs[i];
  return t;
}
(async () => {
  const t0 = Date.now();
  await seq(20000);
  const t1 = Date.now();
  await par(20000);
  const t2 = Date.now();
  console.log("seq_ms", t1 - t0, "par_ms", t2 - t1);
})();
