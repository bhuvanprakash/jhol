// `Generator.prototype.throw`, and a `try` that must survive a suspension.
function* g() {
  try {
    const a = yield 1;
    console.log("got", a);
    return a + 1;
  } catch (e) {
    console.log("caught", e.message);
    return "recovered";
  }
}
const it = g();
console.log("first", it.next().value);
console.log("second", JSON.stringify(it.next(41)));
const it2 = g();
it2.next();
const r = it2.throw(new Error("boom"));
console.log("throw-result", r.value, r.done);

// A throw with no handler inside the body escapes to the caller's try.
function* h() {
  yield 1;
}
const it3 = h();
it3.next();
try {
  it3.throw(new Error("esc"));
} catch (e) {
  console.log("escaped", e.message);
}
