// Replacement strings interpret $&, $`, and $'. A callback preserves bundled
// JavaScript literally, including minifier identifiers such as `$`.
export function embed(template, values) {
  return template.replace(/__(PAYLOAD|MOCK|MANIFEST|FIXTURES)__/g, (_, key) => {
    if (!Object.hasOwn(values, key)) throw Error(`Missing preview value: ${key}`);
    return JSON.stringify(values[key]).replaceAll('<', '\\u003c');
  });
}
