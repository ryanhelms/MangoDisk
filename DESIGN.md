<!-- bytedesk-design-system:start -->
## ByteDesk design inheritance

Read design authority in this order:

1. `.context/design-system/foundation/DESIGN.md`
2. `.context/design-system/apps/mangodisk/DESIGN.md` and adjacent `PRODUCT.md`
3. This repository's root `DESIGN.md` for local implementation details and explicit exceptions

Managed design-system files are read-only. Canonical changes land in `ByteDeskAI/design-system` first.
<!-- bytedesk-design-system:end -->

## Local adapter

This repository may document MangoDisk-specific layout, interaction, and implementation decisions here. Local decisions may specialize the shared profile but must not silently redefine family tokens or another product's personality.

The release pin is `.design-system.json`; `pnpm check:design-system` verifies the
committed scoped tree offline. To update it, install the exact reviewed tokens and
client releases, change the pin, run `pnpm exec design-client sync`, and review the
generated context before committing it. Sync may require registry credentials on
deployments that protect the context package; never commit those credentials.

`src/assets/main.css` imports the published token CSS. The existing 20px shell
padding maps to `--bd-space-7`. `data-bd-product="mangodisk"` identifies the consumer,
and the shared theme stamp follows the application's existing explicit/system
theme choice. MangoDisk's local skins retain their colors, typography and geometry;
this adoption does not replace the GPL application's identity or redesign its UI.
