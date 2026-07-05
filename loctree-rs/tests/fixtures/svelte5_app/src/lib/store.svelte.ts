export const storeName = "counter";
export let count = $state(0);
export const doubled = $derived(count * 2);

export function increment() {
  count += 1;
}
