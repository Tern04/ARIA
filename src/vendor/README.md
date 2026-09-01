# Vendored libraries

The frontend has no bundler (`frontendDist` points straight at `src/`) and the
CSP is `script-src 'self'`, so third-party code is committed here rather than
pulled from a CDN or `node_modules` at runtime.

| File | Package | Version | License |
|---|---|---|---|
| `xterm.mjs`, `xterm.css` | [`@xterm/xterm`](https://www.npmjs.com/package/@xterm/xterm) | 6.0.0 | MIT (`LICENSE.xterm`) |
| `addon-fit.mjs` | [`@xterm/addon-fit`](https://www.npmjs.com/package/@xterm/addon-fit) | 0.11.0 | MIT (`LICENSE.addon-fit`) |

The ESM (`.mjs`) builds are used as-is, minus their trailing
`//# sourceMappingURL=` comment — the `.map` files are 2 MB and would only be
dead weight in the repo.

## Updating

```sh
npm pack @xterm/xterm@<version> @xterm/addon-fit@<version>
```

Then unpack and copy `lib/*.mjs`, `css/xterm.css` and `LICENSE` over the files
above, stripping the `sourceMappingURL` line. Bump the versions in this table.
