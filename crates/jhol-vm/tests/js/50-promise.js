// Promise core + microtask ordering. `async`/`await` are still rejected at
// compile time with a clear message; what works here is then/catch/chaining,
// thenable assimilation, and microtasks draining before timers.
Promise.resolve(1).then(v => console.log("resolved", v));

const p = new Promise(r => r(2));
p.then(v => console.log("executor", v));

Promise.reject(new Error("e")).catch(e => console.log("caught", e.message));

Promise.resolve(1).then(v => v + 1).then(v => console.log("chain", v));

Promise.resolve(3)
  .then(() => { throw new Error("boom"); })
  .catch(e => console.log("threw", e.message));

console.log("sync-first");
queueMicrotask(() => console.log("microtask"));
setTimeout(() => console.log("timer"), 0);

// A promise resolved from inside a thenable is assimilated.
Promise.resolve({ then(res) { res(9); } }).then(v => console.log("thenable", v));
