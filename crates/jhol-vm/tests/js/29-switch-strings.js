function colorName(c) {
  switch (c) {
    case "r": return "red";
    case "g": return "green";
    case "b": return "blue";
    default: return "unknown";
  }
}
console.log(colorName("r"));
console.log(colorName("g"));
console.log(colorName("b"));
console.log(colorName("x"));
const test = ["r", "y", "b"];
for (const t of test) console.log(t + "=" + colorName(t));
