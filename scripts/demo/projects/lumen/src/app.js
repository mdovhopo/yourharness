import { load, save, addHabit, toggle, removeHabit, today } from "./store.js";
import { streak } from "./streak.js";

let state = load();

const list = document.querySelector("#habits");
const empty = document.querySelector("#empty");
const grid = document.querySelector("#grid");
const form = document.querySelector("#new-habit");
const input = document.querySelector("#habit-name");

function commit(next) {
  state = next;
  save(state);
  render();
}

function render() {
  const day = today();
  list.replaceChildren(
    ...state.habits.map((h) => {
      const li = document.createElement("li");
      li.className = h.done.includes(day) ? "habit done" : "habit";
      li.innerHTML = `
        <button class="check" aria-label="Done today">${h.done.includes(day) ? "✓" : ""}</button>
        <span class="name"></span>
        <span class="streak" title="Days in a row">🔥 ${streak(h.done, day)}</span>
        <button class="remove" aria-label="Remove">×</button>`;
      li.querySelector(".name").textContent = h.name;
      li.querySelector(".check").onclick = () => commit(toggle(state, h.id));
      li.querySelector(".remove").onclick = () => commit(removeHabit(state, h.id));
      return li;
    })
  );
  empty.hidden = state.habits.length > 0;
  renderWeek(day);
}

function renderWeek(day) {
  const days = [...Array(7)].map((_, i) => {
    const d = new Date(day);
    d.setDate(d.getDate() - (6 - i));
    return d.toISOString().slice(0, 10);
  });
  grid.replaceChildren(
    ...state.habits.flatMap((h) => [
      Object.assign(document.createElement("span"), { className: "label", textContent: h.name }),
      ...days.map((d) => Object.assign(document.createElement("span"), { className: h.done.includes(d) ? "cell on" : "cell" })),
    ])
  );
}

form.addEventListener("submit", (e) => {
  e.preventDefault();
  if (!input.value.trim()) return;
  commit(addHabit(state, input.value));
  input.value = "";
});

render();
