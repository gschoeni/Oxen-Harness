/** Access the API injected by the Oxen view host. No framework is required. */
export function getViewAPI() {
  if (!globalThis.oxenView || globalThis.oxenView.apiVersion !== 1)
    throw new Error("Open this package in Oxen View Studio or provide a test host.");
  return globalThis.oxenView;
}
