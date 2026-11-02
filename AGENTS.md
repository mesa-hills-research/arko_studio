# Notes for coding agents

Arko Studio is the desktop app of Arko, built on GPUI Kit. CONTRIBUTING.md says how to run the tests and the screenshot goldens, and how to build against local checkouts of the kit, Arko and the framework.

- A change in behaviour comes with a test. Interaction tests drive windows by element ID, and a visual change updates its goldens (`UPDATE_GOLDENS=1 cargo test --locked --test screenshots`), checked in light and dark.
- Tests, fixtures, examples, docs and goldens use made-up paths and names (`/home/ada/src/arko`), never a real machine's or person's.
- Commit subjects are `<area>: <behaviour sentence>` ("settings: every Settings page follows the saves of session names"). When goldens change, the body says which and why. Messages are public, so they say what changed and why for any user of the project.
- Every colour, radius and spacing is a theme token, and every screen reads correctly in all bundled themes, light and dark. Prefer the kit's components and Studio's existing patterns to new ones. When the right fix belongs in the kit or in Arko, make it there.
- The design rules are Arko's `docs/interface-design.md` and the kit's design guides. Draw every state a screen can reach before building it, and show a row or a control only when it earns its place.
- Every dependency comes from crates.io. Local overrides stay in the ignored `.cargo/config.toml` and out of `Cargo.lock`.
