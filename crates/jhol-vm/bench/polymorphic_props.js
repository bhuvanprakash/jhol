let sum = 0;
for (let i = 0; i < 200000; i++) {
  const shape = i % 3;
  let o;
  if (shape === 0) { o = { x: i }; o.w = 1; sum += o.x + o.w; }
  else if (shape === 1) { o = { x: i, y: i }; o.w = 1; sum += o.x + o.y + o.w; }
  else { o = { x: i, y: i, z: i }; o.w = 1; sum += o.x + o.y + o.z + o.w; }
}
console.log(sum);
