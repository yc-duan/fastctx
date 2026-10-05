(() => {
  if (location.href !== "https://world-page.invalid/content" || window === window.top) return;
  const prevent = Event.prototype.preventDefault;
  const apply = Reflect.apply;
  // The engine runs this before page scripts. Keep the cancellation method
  // in the closure: page code can change prototypes, but not this listener.
  navigation.addEventListener("navigate", event => apply(prevent, event, []));
})();
