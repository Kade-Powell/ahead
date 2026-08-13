# Recommended Skills

Audience: AHEAD practitioners

Status: reviewed catalog v0.1

AHEAD may recommend independently maintained agent skills when they strengthen an active phase. Recommendations are references, not bundled code or silent dependencies. A human chooses whether to inspect and install one, and AHEAD's workflow, authority boundaries, and gates remain controlling.

The machine-readable catalog is `recommendations/skills-v0.1.json`. Every entry pins the exact revision AHEAD reviewed, supplies an inspectable source URL and install command, identifies applicable workflows and phases, and records compatibility constraints. Updating a recommendation requires reviewing the new revision before changing the pin.

## Ponytail

[Ponytail](https://github.com/DietrichGebert/ponytail/tree/2ed6c52c9d7e5e56942508591085fd45dea277d3/skills/ponytail) is recommended as an optional persistence and problem-solving aid for difficult implementation, corrective debugging, and investigation work. AHEAD does not adopt Ponytail's authority model: the human still leads, its suggestions remain subject to the active phase, and a quick version is not production-ready merely because it works.

Inspect the pinned source, then opt in with:

```sh
npx skills add https://github.com/DietrichGebert/ponytail/tree/2ed6c52c9d7e5e56942508591085fd45dea277d3/skills/ponytail --agent pi
```

The Pi integration exposes this catalog but never runs the command itself. Future editor adapters should render the same canonical catalog through their native UI.
