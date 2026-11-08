(() => {
  const $ = (selector) => document.querySelector(selector);
  const plate = $("#plate");
  const frame = $("#demo-frame");
  const live = $("#live");
  const mobileInput = $("#mobile-input");
  let timer = 0;
  let retryUsed = false;
  let ended = false;
  let controlHeld = false;
  let activeRenderer = "webgl2";
  let rendererFallback = false;
  let composing = false;
  let suppressCommittedInput = "";

  function announce(message) {
    live.textContent = "";
    window.setTimeout(() => { live.textContent = message; }, 30);
  }

  function show(state, focus = false) {
    window.clearTimeout(timer);
    plate.dataset.state = state;
    document.querySelectorAll("[data-show]").forEach((element) => {
      element.hidden = !element.dataset.show.split(" ").includes(state);
    });
    if (focus) {
      const target = {
        poster: $("#load"),
        loading: $("#cancel"),
        running: frame,
        released: $("#scrim"),
        unsupported: $("#unsupported-title"),
        failed: $("#retry"),
      }[state];
      target?.focus({ preventScroll: true });
    }
  }

  function post(message) {
    frame.contentWindow?.postMessage(message, location.origin);
  }

  function focusTerminal() {
    show("running");
    frame.contentWindow?.focus();
    try {
      const target = frame.contentDocument?.querySelector("#demo canvas, #demo_ratzilla_grid")
        || frame.contentDocument?.getElementById("demo");
      target?.focus();
    } catch {
      // Same-origin is required for the shell bridge; message origin checks reject others.
    }
    announce("Demo ready. Keyboard captured.");
  }

  function releaseKeyboard() {
    if (plate.dataset.state !== "running") return;
    frame.blur();
    show("released", true);
    announce("Keyboard released. Page scroll and Tab work as usual.");
  }

  function start(renderer = "webgl2", retry = false) {
    if (typeof WebAssembly === "undefined") {
      show("unsupported", true);
      announce("This demo needs WebAssembly. Your browser does not provide it.");
      return;
    }
    if (!retry) {
      retryUsed = false;
      activeRenderer = renderer;
      rendererFallback = false;
      $("#poster-note")?.remove();
    }
    $("#renderer-note").hidden = true;
    ended = false;
    frame.hidden = true;
    frame.removeAttribute("src");
    show("loading", true);
    frame.onload = () => {
      if (!frame.contentWindow || frame.contentWindow.location.origin !== location.origin) {
        fail("The terminal frame did not load from this site.");
      }
    };
    timer = window.setTimeout(() => fail("The demo did not become ready in time."), 30_000);
    frame.hidden = false;
    frame.src = `./demo.html?renderer=${activeRenderer}`;
  }

  function fail(reason) {
    if (plate.dataset.state !== "loading") return;
    window.clearTimeout(timer);
    frame.removeAttribute("src");
    frame.hidden = true;
    if (retryUsed) {
      $("#failure-copy").textContent = `${reason} Retry failed too. The static poster is still available.`;
      show("poster", true);
      const note = document.createElement("p");
      note.className = "failure-note";
      note.setAttribute("role", "alert");
      note.textContent = "The interactive demo failed twice. Use the static poster above.";
      $("#poster-note")?.remove();
      note.id = "poster-note";
      $(".poster").append(note);
      announce(note.textContent);
      return;
    }
    $("#failure-copy").textContent = reason;
    show("failed", true);
    announce("Demo failed to load.");
  }

  function sendKey(code, modifiers = 0) {
    if (plate.dataset.state === "released" && !ended) focusTerminal();
    if (plate.dataset.state === "running") post({ type: "key", code, modifiers });
  }

  function sendText(text) {
    for (const character of text) sendKey(character);
    controlHeld = false;
    $("[data-modifier='Control']").setAttribute("aria-pressed", "false");
  }

  $("#load").addEventListener("click", () => start());
  $("#retry").addEventListener("click", () => {
    retryUsed = true;
    start(activeRenderer, true);
  });
  $("#cancel").addEventListener("click", () => {
    window.clearTimeout(timer);
    frame.removeAttribute("src");
    frame.hidden = true;
    show("poster", true);
    announce("Loading cancelled. Nothing was saved.");
  });
  $("#reset").addEventListener("pointerdown", (event) => event.preventDefault());
  $("#reset").addEventListener("click", () => {
    ended = false;
    post({ type: "reset" });
    show("running");
    frame.focus();
    post({ type: "focus" });
    announce("Demo reset. The fictional chats are back to their starting state.");
  });
  $("#release").addEventListener("click", releaseKeyboard);
  $("#scrim").addEventListener("click", () => {
    if (ended) {
      start();
      return;
    }
    focusTerminal();
  });
  frame.addEventListener("blur", () => {
    window.setTimeout(() => {
      if (plate.dataset.state === "running" && document.activeElement !== mobileInput) {
        show("released", true);
        announce("Keyboard released.");
      }
    });
  });

  window.addEventListener("keydown", (event) => {
    if (plate.dataset.state !== "running") return;
    if (event.ctrlKey && event.altKey && event.key === "Escape" && document.activeElement !== frame) {
      event.preventDefault();
      releaseKeyboard();
    }
  });
  window.addEventListener("message", (event) => {
    if (event.origin !== location.origin || event.source !== frame.contentWindow) return;
    switch (event.data?.type) {
      case "ready":
        window.clearTimeout(timer);
        performance.mark("televim-ready");
        focusTerminal();
        if (rendererFallback) {
          $("#renderer-note").hidden = false;
          announce("WebGL2 did not start. The accessible DOM renderer is active.");
        }
        break;
      case "failed":
        fail(event.data.message || "The demo could not load.");
        break;
      case "release":
        if (plate.dataset.state === "running") {
          frame.blur();
          show("released", true);
          announce("Keyboard released. Page scroll and Tab work as usual.");
        }
        break;
      case "quit-demo":
        ended = true;
        $("#interact-label").textContent = "Load demo again";
        $("#released-copy").textContent = "Demo ended. Load again or Reset.";
        show("released", true);
        announce("Demo ended. Load again or Reset.");
        break;
      case "renderer-fallback":
        rendererFallback = true;
        break;
    }
  });

  $("#accessible-renderer").addEventListener("click", (event) => {
    event.preventDefault();
    start("dom");
  });

  document.querySelectorAll("[data-key]").forEach((button) => {
    button.addEventListener("pointerdown", (event) => event.preventDefault());
    button.addEventListener("click", () => {
      sendKey(button.dataset.key, controlHeld ? 1 : 0);
      controlHeld = false;
      $("[data-modifier='Control']").setAttribute("aria-pressed", "false");
    });
  });
  $("[data-modifier='Control']").addEventListener("click", (event) => {
    controlHeld = !controlHeld;
    event.currentTarget.setAttribute("aria-pressed", String(controlHeld));
  });
  $("[data-modifier='Control']").addEventListener("pointerdown", (event) => event.preventDefault());
  $("#touch-done").addEventListener("click", () => {
    mobileInput.blur();
    releaseKeyboard();
  });
  $("#mobile-type").addEventListener("click", () => mobileInput.focus());
  mobileInput.addEventListener("compositionstart", () => {
    composing = true;
    suppressCommittedInput = "";
  });
  mobileInput.addEventListener("compositionupdate", () => {
    composing = true;
  });
  mobileInput.addEventListener("compositionend", (event) => {
    composing = false;
    const committed = event.data;
    if (committed) {
      sendText(committed);
      suppressCommittedInput = committed;
      window.setTimeout(() => {
        if (suppressCommittedInput === committed) suppressCommittedInput = "";
      }, 0);
    }
    mobileInput.value = "";
  });
  mobileInput.addEventListener("input", (event) => {
    const text = mobileInput.value;
    if (composing || event.isComposing || !text) return;
    if (text === suppressCommittedInput) {
      suppressCommittedInput = "";
    } else {
      suppressCommittedInput = "";
      sendText(text);
    }
    mobileInput.value = "";
  });
  mobileInput.addEventListener("focus", () => {
    if (plate.dataset.state === "released" && !ended) show("running");
  });
  mobileInput.addEventListener("beforeinput", (event) => {
    if (composing || event.isComposing) return;
    if (event.inputType === "deleteContentBackward") {
      event.preventDefault();
      sendKey("Backspace");
    }
  });
  mobileInput.addEventListener("keydown", (event) => {
    if (composing || event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      sendKey("Enter");
    }
  });

  fetch("./poster.txt")
    .then((response) => response.ok ? response.text() : Promise.reject(new Error("poster unavailable")))
    .then((poster) => { $("#poster-frame").textContent = poster; })
    .catch(() => { $("#poster-frame").textContent = "Static poster unavailable."; });
})();
