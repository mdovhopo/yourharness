// Habits and check-ins, kept in localStorage.

const KEY = "lumen.v1";

export function load() {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? JSON.parse(raw) : { habits: [] };
  } catch {
    return { habits: [] };
  }
}

export function save(state) {
  localStorage.setItem(KEY, JSON.stringify(state));
}

export function addHabit(state, name) {
  const habit = { id: crypto.randomUUID(), name: name.trim(), created: today(), done: [] };
  return { ...state, habits: [...state.habits, habit] };
}

export function toggle(state, id, day = today()) {
  return {
    ...state,
    habits: state.habits.map((h) =>
      h.id !== id ? h : { ...h, done: h.done.includes(day) ? h.done.filter((d) => d !== day) : [...h.done, day] }
    ),
  };
}

export function removeHabit(state, id) {
  return { ...state, habits: state.habits.filter((h) => h.id !== id) };
}

export function today(date = new Date()) {
  return date.toISOString().slice(0, 10);
}
