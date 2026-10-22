const name = "Sam";
const age = 30;
console.log(`Hello, ${name}!`);
console.log(`${name} is ${age}`);
console.log(`${age > 18 ? "adult" : "minor"}`);
console.log(`Sum: ${1 + 2}`);
const nested = `outer ${`inner ${age}`}`;
console.log(nested);
function tag(strings, ...values) { return strings.join("|") + "#" + values.join(","); }
console.log(tag`a${1}b${2}c`);
