# Portfolio embed contract

The portfolio repo owns the page integration. Televim repo publishes a static
shell at `https://curedbylethe.github.io/televim/`; the shell loads WebAssembly
only after its own **Load interactive demo** action.

Keep a local static poster and provenance label visible before visitors opt in.
The poster asset path below is owned by the portfolio repo; create it from the
Televim poster or a reviewed recording frame. No recording asset exists in this
repo yet.

```html
<figure class="televim-demo">
  <img
    src="/portfolio/assets/televim-poster.png"
    alt="Televim terminal showing fictional conversations"
  >
  <figcaption>Interactive Rust/WASM demo · fictional conversations · offline</figcaption>
  <p>Runs Televim's real Rust interface with fictional data. It does not connect to Telegram.</p>
  <button type="button" data-load-televim>Load interactive demo</button>
  <div data-televim-frame></div>
  <a href="https://curedbylethe.github.io/televim/" target="_blank" rel="noopener noreferrer">
    Open the Televim demo page
  </a>
</figure>

<script>
  document.querySelector("[data-load-televim]").addEventListener("click", (event) => {
    const figure = event.currentTarget.closest(".televim-demo");
    const frame = document.createElement("iframe");
    frame.src = "https://curedbylethe.github.io/televim/";
    frame.title = "Televim interactive demo";
    frame.loading = "lazy";
    frame.allow = "clipboard-write";
    frame.referrerPolicy = "no-referrer";
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin allow-downloads");
    figure.querySelector("[data-televim-frame]").replaceChildren(frame);
    event.currentTarget.disabled = true;
  }, { once: true });
</script>
```

The portfolio owns its CSP and must allow the Televim Pages origin in
`frame-src`. Televim's meta CSP allows same-origin internal assets and frames;
GitHub Pages cannot set `frame-ancestors` response headers. Keep `allow-same-origin`
on the cross-origin iframe so Televim's own same-origin iframe and message
bridge work. The outer iframe remains cross-origin from the portfolio page.

The shell inside the frame owns **Load interactive demo**, **Reset demo** and
**Release keyboard**. Do not duplicate these controls in the portfolio. Keep the
provenance label outside the frame before and after load.

## Loading and fallback

- Do not create the iframe or set its `src` before the visitor clicks. Do not preload the WASM file.
- `loading="lazy"` is an additional hint; delaying `src` until the click is the load guarantee.
- Keep the poster and a direct link to Televim Pages available if the iframe cannot load.
- The Televim shell reports module failure and keeps its own static poster available.
- Set a useful iframe `title`; keep the poster alt text descriptive.

## Cross-repo checklist

- Add the portfolio-owned poster asset and embed to `project-televim.html`.
- Allow `https://curedbylethe.github.io` in the portfolio's `frame-src` policy.
- Verify the deployed `/televim/` base path and browser MIME handling.
- Test keyboard release, mobile layout, Reset and clipboard permission.
- Keep one provenance label outside the iframe; do not imply a Telegram connection.
- Link a recording only after the portfolio repo has a reviewed recording asset.

**AC6 status:** Televim repo supplies the Pages workflow, `/televim/` base path
and this contract. The portfolio PR and live embed check remain pending; this
repo does not deploy changes to the portfolio.
