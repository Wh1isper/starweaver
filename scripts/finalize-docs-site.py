#!/usr/bin/env python3
"""Write deployment metadata for the mdBook documentation site."""

from __future__ import annotations

import html
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BOOK_DIR = ROOT / "book"
SITE_URL = "https://starweaver.wh1isper.top"
EXCLUDED_HTML = {"404.html", "toc.html"}


def page_url(path: Path) -> str:
    relative = path.relative_to(BOOK_DIR).as_posix()
    if relative == "index.html":
        return f"{SITE_URL}/"
    return f"{SITE_URL}/{relative}"


def sitemap_urls() -> list[str]:
    pages = []
    for path in sorted(BOOK_DIR.rglob("*.html")):
        if path.name in EXCLUDED_HTML:
            continue
        pages.append(page_url(path))
    return pages


def write_sitemap(urls: list[str]) -> None:
    lines = [
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
    ]
    lines.extend(f"  <url><loc>{html.escape(url)}</loc></url>" for url in urls)
    lines.append("</urlset>")
    (BOOK_DIR / "sitemap.xml").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_robots() -> None:
    (BOOK_DIR / "robots.txt").write_text(
        "User-agent: *\n"
        "Allow: /\n"
        f"Sitemap: {SITE_URL}/sitemap.xml\n",
        encoding="utf-8",
    )


def main() -> None:
    if not BOOK_DIR.exists():
        raise SystemExit("book directory does not exist; run mdbook build first")
    urls = sitemap_urls()
    write_sitemap(urls)
    write_robots()
    print(f"Wrote sitemap.xml with {len(urls)} URLs and robots.txt")


if __name__ == "__main__":
    main()
