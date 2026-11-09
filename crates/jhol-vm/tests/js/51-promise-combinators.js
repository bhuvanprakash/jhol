// Promise combinators and finally. `Promise.all/allSettled/race/finally` were
// undefined; they run on the same microtask queue as `then`, so ordering has to
// match node, not merely the results.
Promise.all([1, Promise.resolve(2), 3]).then(v => console.log("all", v.join(",")));
Promise.all([1, Promise.reject(new Error("x"))]).then(
  () => console.log("should not run"),
  e => console.log("all-rej", e.message)
);
Promise.allSettled([Promise.resolve(1), Promise.reject(new Error("y"))]).then(
  rs => console.log("settled", rs.map(r => r.status).join(","), rs[0].value)
);
Promise.race([
  new Promise(r => setTimeout(() => r("slow"), 10)),
  Promise.resolve("fast"),
]).then(v => console.log("race", v));
Promise.resolve(5).finally(() => console.log("fin")).then(v => console.log("after-fin", v));
Promise.reject(new Error("z"))
  .finally(() => console.log("fin2"))
  .catch(e => console.log("caught", e.message));
console.log("sync");
