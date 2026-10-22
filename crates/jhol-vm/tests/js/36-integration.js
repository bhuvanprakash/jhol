const people = [
  { name: "Charlie", age: 35 },
  { name: "Alice", age: 30 },
  { name: "Bob", age: 35 },
  { name: "Dave", age: 25 }
];
people.sort((a, b) => a.age - b.age || a.name.localeCompare(b.name));
const ages = people.map(p => p.age);
const total = ages.reduce((s, x) => s + x, 0);
const stats = { min: Math.min(...ages), max: Math.max(...ages), avg: total / ages.length };
console.log("min=" + stats.min);
console.log("max=" + stats.max);
console.log("avg=" + stats.avg);
console.log("Name".padStart(8) + "|" + "Age".padStart(5));
for (const p of people) {
  console.log(p.name.padStart(8) + "|" + String(p.age).padStart(5));
}
console.log("total=" + total);
