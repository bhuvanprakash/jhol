// Temporal dead zone (statically decidable cases) and arrow-function bindings.
// Temporal dead zone (statically decidable cases) and arrow-function
// an arrow body panicked the compiler because arrows were pushed with no scope.
const out = [];

// Arrow bodies may declare bindings of every kind.
out.push((() => { let z = 3; return z + 1 })());
out.push((() => { const { a, b } = { a: 1, b: 2 }; return a + b })());
out.push((() => { var v = 7; return v })());
out.push((() => { let n = 0; for (const x of [1, 2, 3]) n += x; return n })());

// Reading a `let` in its own initializer throws, and names the binding.
function tdzInInit() {
  try { let a = a; return "no throw"; }
  catch (e) { return e.message; }
}
out.push(tdzInInit());

// Reading a block-scoped name before its declaration throws too.
function tdzInBlock() {
  try { { q; let q = 1; } return "no throw"; }
  catch (e) { return e.message; }
}
out.push(tdzInBlock());

// Legal cases stay legal: reading after the declaration, and a closure that
// runs after it.
function legalAfter() {
  let a = 1;
  let b = a + 1;
  return b;
}
out.push(legalAfter());
function legalClosure() {
  const f = function () { return x; };
  let x = 5;
  return f();
}
out.push(legalClosure());

// `var` is hoisted and initialised to undefined: no dead zone.
function varHoisted() {
  var v = v;
  return v === undefined;
}
out.push(varHoisted());

console.log(out.join(" "));
