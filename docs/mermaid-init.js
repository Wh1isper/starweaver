(() => {
  const script = document.createElement("script");
  script.type = "module";
  script.textContent = `
    import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
    mermaid.initialize({ startOnLoad: false, securityLevel: "strict" });

    for (const block of document.querySelectorAll("pre code.language-mermaid")) {
      const container = document.createElement("div");
      container.className = "mermaid";
      container.textContent = block.textContent;
      block.closest("pre").replaceWith(container);
    }

    await mermaid.run({ querySelector: ".mermaid" });
  `;
  document.head.appendChild(script);
})();
