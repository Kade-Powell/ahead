import assert from "node:assert/strict";
import test from "node:test";
import {
  loadRecommendedSkills,
  recommendedSkillsMarkdown,
  relevantRecommendedSkills,
} from "../src/skills.ts";

test("recommended skills are pinned, inspectable, and opt-in", async () => {
  const catalog = await loadRecommendedSkills();
  const ponytail = catalog.skills.find((skill) => skill.id === "ponytail");
  assert.ok(ponytail);
  assert.match(ponytail.reviewed_ref, /^[a-f0-9]{40}$/);
  assert.match(ponytail.reviewed_url, new RegExp(ponytail.reviewed_ref));
  assert.match(ponytail.install, new RegExp(ponytail.reviewed_ref));
  assert.match(ponytail.install, /--agent pi/);
  assert.ok(ponytail.compatibility.some((rule) => /AHEAD remains authoritative/.test(rule)));

  const markdown = recommendedSkillsMarkdown(catalog, [ponytail]);
  assert.match(markdown, /does not bundle or install/i);
  assert.match(markdown, /Optional install after you inspect the source/);
});

test("skill recommendations are scoped to applicable workflow phases", async () => {
  const catalog = await loadRecommendedSkills();
  assert.deepEqual(
    relevantRecommendedSkills(catalog, "corrective-debugging", "investigate").map(
      (skill) => skill.id,
    ),
    ["ponytail"],
  );
  assert.deepEqual(relevantRecommendedSkills(catalog, "decision", "decide"), []);
});
