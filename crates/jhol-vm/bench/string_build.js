let s = "";
for (let i = 0; i < 200000; i++) s += (i % 10).toString();
const idx = s.indexOf("999");
s.slice(0, 32);
let h = 0;
for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 2147483647;
console.log(s.length + " " + idx + " " + h);
