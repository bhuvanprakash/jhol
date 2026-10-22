try { throw new Error("boom"); } catch (e) { console.log(e.message); }
function risky(n) {
  try {
    if (n < 0) throw new Error("negative");
    return n * 2;
  } catch (e) {
    return "err:" + e.message;
  } finally {
  }
}
console.log(risky(5));
console.log(risky(-1));
let order = "";
try {
  order += "t";
  try { order += "i"; throw new Error("x"); } catch (e) { order += "c"; } finally { order += "f"; }
} catch (e) {
  order += "o";
} finally {
  order += "F";
}
console.log(order);
function rethrow() { try { throw new Error("orig"); } catch (e) { throw e; } }
try { rethrow(); } catch (e) { console.log("caught " + e.message); }
