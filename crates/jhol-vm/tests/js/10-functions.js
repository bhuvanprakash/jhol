function add(a, b) { return a + b; }
console.log(add(2, 3));
function greet(name) { return "Hi " + name; }
console.log(greet("Sam"));
function withDefault(a, b) { b = b || 10; return a + b; }
console.log(withDefault(5));
console.log(withDefault(5, 20));
function noReturn(x) { console.log("side" + x); }
noReturn(7);
const mul = function(a, b) { return a * b; };
console.log(mul(3, 4));
const arrow = (a, b) => a - b;
console.log(arrow(9, 4));
