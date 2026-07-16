# AI Writing Patterns — Distilled Field Guide

Distilled from Wikipedia's "Signs of AI writing" (WikiProject AI Cleanup). The full catalogue with real examples and research citations lives in `signs-of-ai-writing.md`; load that only for an explicit "is this text AI-written?" audit.

The root cause of every pattern below: LLMs regress to the mean. Specific, unusual facts get smoothed into generic, important-sounding claims — the subject becomes less specific and more exaggerated at the same time. The fix is always some form of the same move: delete the claim of significance and state the specific fact.

## Content patterns

**Inflated importance.** Watch: *stands/serves as a testament, plays a vital/crucial/pivotal role, underscores/highlights its importance, reflects broader, enduring/lasting legacy, key turning point, indelible mark, deeply rooted, rich heritage*. Arbitrary aspects of the topic get framed as contributing to some broader significance. Fix: state the fact; cut the significance clause.

**Superficial -ing analyses.** Watch: sentence-final *…, ensuring/highlighting/emphasizing/reflecting/underscoring/showcasing/aligning with/contributing to…*. Strongest tell: the subject is a fact or event — a person can highlight something; a fact cannot. These are unattributed opinions by a disembodied narrator. Fix: end the sentence at the fact, or attribute the opinion to a named source that actually holds it.

**Promotional tone.** Watch: *nestled, in the heart of, boasts, stunning, breathtaking, vibrant, groundbreaking, seamless, gateway to, continues to captivate*. Reads like ad copy or a TV commercial transcript. Fix: neutral, concrete description.

**Didactic disclaimers.** Watch: *it's important/crucial to note/remember/consider, may vary, always check*. Lecture-to-the-reader asides. Fix: cut; trust the reader.

**Formulaic wrap-ups.** Watch: *In summary, In conclusion, Overall*; the rigid "Despite [challenges], [subject] continues to…" / "Future Outlook" outline. Paragraphs restate their own core idea at the end. Fix: end when the content ends.

**Vague attribution.** Watch: *industry reports, observers have cited, some critics argue, has been described as*. Weasel-worded opinions attributed to no one, or one thin source overgeneralized into a chorus. Fix: name the source or drop the claim.

**Knowledge-cutoff and gap disclaimers.** Watch: *as of my last training update, while specific details are limited/not widely documented, based on available information, …likely…*. Speculation dressed as diligence. Fix: delete; write only what is known.

**Chat leakage.** Watch: *I hope this helps, Certainly!, Would you like…, let me know, here is a…*; unfilled placeholders (*[Name]*, *2025-XX-XX*). Correspondence pasted as content. Fix: delete wholesale.

## Language patterns

**AI vocabulary.** The heavily overused set: *delve, leverage, foster, robust, seamless(ly), crucial, vital, key (adj.), multifaceted, tapestry, landscape, realm, garner, underscore, showcase, testament, intricate, nuanced, pivotal, enhance, streamline, notably, vibrant, interplay, align with, shed light on, enduring, boast*. These co-occur: where there is one, there are likely others. One instance is noise; clusters are the tell. Fix: the plain word a knowledgeable human would pick.

**Negative parallelism.** Watch: *not only … but …, it's not just about …, it's …, no …, no …, just …*. Fix: say the one thing that is true, without the shadow-boxing.

**Rule of three.** Triads everywhere — *adjective, adjective, adjective; phrase, phrase, and phrase* — used to make thin analysis look comprehensive. Fix: keep the one or two items that carry weight.

**Elegant variation.** Synonym-cycling to avoid repeating a name (*the protagonist, the key player, the eponymous character*). Fix: repeat the name; repetition is clearer than variation.

**False ranges.** *From X to Y* where no scale connects the endpoints ("from problem-solving to artistic expression"). Test: can you name a midpoint on one scale without switching scales? If not, it's decoration. Fix: list the items plainly or cut.

## Formatting patterns

**Boldface overuse.** Every **key term** bolded, "key takeaways" style. Fix: bold nothing, or one genuinely load-bearing term.

**Inline-header lists.** Bullets shaped `- **Header:** description text` repeated down the page. Fix: prose, or a plain list.

**Emoji decoration.** Emojis fronting headings or bullets. Fix: delete.

**Em-dash overuse.** Em dashes used formulaically where commas, colons, or parentheses would do — especially in punched-up parallelisms. Fix: default to the quieter mark.

**Title Case Headings.** LLMs capitalize All Main Words in headings. Fix: sentence case.
