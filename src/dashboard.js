"use strict";
(() => {
  const status = document.getElementById("refresh-status");
  let lastSuccess = 0;
  let failed = false;
  let partial = false;
  let pending = false;

  function freshness() {
    const stale = lastSuccess && Date.now() - lastSuccess > 15000;
    status.className = failed || stale || partial ? "timestamp attention" : "timestamp";
    const last = lastSuccess
      ? ` Last update received: ${new Date(lastSuccess).toLocaleString()}.`
      : " No data received yet.";
    const message = failed || stale
      ? lastSuccess
        ? `Update failed or delayed — showing older data.${last} Retrying automatically.`
        : "Unable to obtain status. No data received yet. Retrying automatically."
      : partial
        ? `Connected — some status data is unavailable.${last}`
        : lastSuccess
          ? `Live — refreshes every 5 seconds.${last}`
          : "Connecting to the local viewer…";
    if (status.textContent !== message) status.textContent = message;
  }

  async function refresh() {
    if (pending) return;
    pending = true;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    try {
      const response = await fetch("/snapshot", {
        cache: "no-store",
        credentials: "omit",
        signal: controller.signal,
      });
      if (!response.ok) throw new Error("Status unavailable");
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
      const main = page.querySelector("main");
      if (!main) throw new Error("Invalid snapshot");
      partial = !!main.querySelector("[data-unavailable]");
      document.querySelector("main").replaceChildren(status, ...main.childNodes);
      lastSuccess = Date.now();
      failed = false;
    } catch {
      failed = true;
    } finally {
      clearTimeout(timeout);
      pending = false;
      freshness();
      setTimeout(refresh, 5000);
    }
  }

  document.addEventListener("visibilitychange", freshness);
  setInterval(freshness, 1000);
  refresh();
})();
