// How many days in a row a habit was done, counting back from `day`.

export function streak(done, day) {
  const set = new Set(done);
  let count = 0;
  let cursor = new Date(day);
  while (set.has(cursor.toISOString().slice(0, 10))) {
    count++;
    cursor.setDate(cursor.getDate() - 1);
  }
  return count;
}

// The longest run ever.
export function longest(done) {
  const days = [...new Set(done)].sort();
  let best = 0;
  let run = 0;
  let prev = null;
  for (const d of days) {
    const date = new Date(d);
    if (prev && (date - prev) / 86400000 === 1) {
      run++;
    } else {
      run = 1;
    }
    best = Math.max(best, run);
    prev = date;
  }
  return best;
}
