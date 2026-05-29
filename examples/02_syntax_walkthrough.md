# Syntax Walkthrough

This slide is meant to show the richer markdown renderer in a way that feels
closer to a live walkthrough than a plain terminal document.

> [!NOTE]
> The code blocks below should show real syntax highlighting while still living
> inside the presentation surface.

## Python

```python
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Slide:
    title: str
    body: str


def load_slide(path: str) -> Slide:
    source = Path(path).read_text().strip().splitlines()
    title = source[0].removeprefix("# ")
    body = "\n".join(source[1:]).strip()
    return Slide(title=title, body=body)


slide = load_slide("examples/02_syntax_walkthrough.md")
print(f"loaded: {slide.title}")
```

## Shell

```bash
set -euo pipefail

deck_dir="./examples"
slide_count="$(rg --files "$deck_dir" -g '*.md' | wc -l | tr -d ' ')"

printf 'slides: %s\n' "$slide_count"
printf 'launching viewer...\n'
make run ARGS="$deck_dir"
```

## Takeaways

- **Strong** text should stand out cleanly.
- *Emphasis* should read softly, not like plain body text.
- Inline `code` should feel intentionally boxed.
- Links like [syntect](https://github.com/trishume/syntect) should be visibly distinct.

> [!TIP]
> If a code-heavy deck feels cramped, try mixing these walkthrough slides with
> image or section slides so the rhythm stays presentation-like.
