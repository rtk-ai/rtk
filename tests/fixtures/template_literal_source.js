const plain = `multi
line # template`;

const rendered = `header
// This is literal text, including the backtick escape: \`
/* This is also literal text.


Its blank lines belong to the value. */
${(() => {
    const quoted = "`";
    const pattern = /[`}]/;
    // A comment inside interpolation can mention ` and }.
    return `nested ${pattern.test(quoted)}
    # nested content`;
})()}
footer`;

function tagged(parts) {
    return parts.raw[0];
}

const raw = tagged`escaped delimiter: \`
closing line`;

console.log(plain, rendered, raw);
