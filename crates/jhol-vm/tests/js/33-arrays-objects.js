const people = [
  { name: "Alice", age: 30 },
  { name: "Bob", age: 25 },
  { name: "Carol", age: 30 }
];
const sorted = [...people].sort((a, b) => a.age - b.age || a.name.localeCompare(b.name));
console.log(sorted.map(p => p.name).join(","));
const total = people.reduce((s, p) => s + p.age, 0);
console.log(total);
const adults = people.filter(p => p.age >= 30).map(p => p.name.toUpperCase());
console.log(adults.join(","));
const byName = people.find(p => p.name === "Bob");
console.log(byName.age);
console.log(people.map(p => p.age).join("+"));
