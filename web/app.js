if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {
    // Offline caching is an enhancement; the monitor remains usable without it.
  });
}
