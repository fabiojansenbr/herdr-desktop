// Test-only reactive holder (Svelte 5 runes). Component tests of the composed window need the
// same wiring as App.svelte: the controller's onChange writes a `$state` that the props getters
// read, so a bridge event re-renders the component under test. Not part of the product bundle.

export interface ReactiveHolder<T> {
  value: T;
}

export function reactiveHolder<T>(initial: T): ReactiveHolder<T> {
  const state = $state({ value: initial });
  return {
    get value() {
      return state.value;
    },
    set value(next: T) {
      state.value = next;
    },
  } as ReactiveHolder<T>;
}
