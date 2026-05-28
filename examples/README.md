# Examples

Example slides for `ss`.

Files:

- `00_intro.md`: normal text-plus-image slide
- `01_image_layout.md`: image-focused slide using frontmatter

Image path convention used by both slides:

```md
![example image](./pictures/image.png)
```

Add a real PNG at `./pictures/image.png` and run:

```sh
make run ARGS=./examples
```
