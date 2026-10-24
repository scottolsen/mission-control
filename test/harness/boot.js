/**
 * Loads the real src/index.html into the harness page: copies its <head>
 * stylesheets and <body> markup (paths rewritten to ../../src/), then runs its
 * scripts in order. The markup is never duplicated here, so the harness can't
 * drift from the app.
 *
 * Guarded so `node --test` (which globs test/**) can load it harmlessly.
 */
if (typeof window !== "undefined") {
  const SRC = new URL("../../src/", document.baseURI);
  const html = await (await fetch(new URL("index.html", SRC))).text();
  const doc = new DOMParser().parseFromString(html, "text/html");

  document.title = `${doc.title} — harness`;
  for (const link of doc.head.querySelectorAll('link[rel="stylesheet"]')) {
    const l = document.createElement("link");
    l.rel = "stylesheet";
    l.href = new URL(link.getAttribute("href"), SRC).href;
    document.head.append(l);
    await new Promise((r) => { l.onload = l.onerror = r; });
  }

  const scripts = [...doc.body.querySelectorAll("script")];
  for (const s of scripts) s.remove();
  document.body.replaceChildren(...[...doc.body.childNodes].map((n) => document.importNode(n, true)));

  for (const s of scripts) {
    const src = new URL(s.getAttribute("src"), SRC).href;
    if (s.type === "module") {
      await import(src);
    } else {
      const el = document.createElement("script");
      el.src = src;
      document.body.append(el);
      await new Promise((r, j) => { el.onload = r; el.onerror = j; });
    }
  }
}
