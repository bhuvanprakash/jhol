const list = [];
for (let i = 0; i < 5000; i++) {
  list.push({
    id: i,
    name: "it" + i,
    tags: [i % 2 === 0, i % 3 === 0],
    nest: { v: i * 3, w: i * i },
  });
}
const text = JSON.stringify({ count: list.length, list: list });
const back = JSON.parse(text);
let checksum = 0;
for (let i = 0; i < back.list.length; i++) {
  const e = back.list[i];
  checksum = (checksum + e.id + e.nest.v + e.nest.w) % 4294967296;
}
console.log(back.count + " " + checksum);
