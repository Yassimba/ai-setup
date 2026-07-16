---
name: good-writing
description: Write clear, human-sounding prose and generously cite sources with links.
type: prompt
whenToUse: When writing or editing customer-facing copy, documentation, emails, case studies, proposals, blog posts, Slack announcements, or any prose that will be read by humans.
disableModelInvocation: false
---

# Good Writing

Use this skill when writing or editing anything humans will read: website copy, docs, emails, case studies, proposals, blog posts, Slack announcements, etc.

## Rules

1. **Sound like a person, not a brochure.** Use contractions, active voice, and short sentences.
2. **Cut AI filler.** Avoid: delve, landscape, tapestry, robust, seamless, cutting-edge, transformative, pioneering, leverage, in today's world, it's important to note, ultimately, moreover, furthermore.
3. **One idea per sentence.** If a sentence has more than one comma, split it.
4. **Be specific.** Say what happened, not what "represents a paradigm shift."
5. **No em dashes.** Use periods, commas, or parentheses.
6. **Generously cite sources.** When you mention a fact, standard, tool, article, repo, or prior decision, link to it.
   - Prefer primary sources: official docs, the actual file in the repo, the PR, the issue, the design doc.
   - Use absolute `/workspace/...` paths when citing files inside the container.
   - Use full URLs for external sources.
   - Place links inline or in a "Sources" section — whichever reads better.
7. **Defer to the project's voice.** If the repo has a style guide or brand notes, follow those first.
