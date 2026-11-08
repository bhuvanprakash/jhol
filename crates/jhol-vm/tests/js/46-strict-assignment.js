// Strict code may not create a global by assignment; sloppy code may.
// The two questions are separate: is the frame strict, and is it the *script*?
// node runs `-e`/script-level code as sloppy, so a script must not throw here.
const out = [];

function strictFn() {
  "use strict";
  try { undeclared_zzz = 1; return "no throw"; }
  catch (e) { return e.message; }
}
out.push(strictFn());

function sloppyFn() {
  sloppy_created = 41;
  return sloppy_created + 1;
}
out.push(sloppyFn());

class StrictByDefault {
  m() {
    try { undeclared_qqq = 1; return "no throw"; }
    catch (e) { return e.message; }
  }
}
out.push(new StrictByDefault().m());

globalThis.existing_www = 1;
function strictWritesExisting() {
  "use strict";
  existing_www = 2;
  return existing_www;
}
out.push(strictWritesExisting());

// Script level is sloppy: this must not throw.
script_level = 7;
out.push(script_level);

console.log(out.join(" "));
