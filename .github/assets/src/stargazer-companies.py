"""Where the logo strip's third row and its count come from.

    python3 stargazer-companies.py harvest stargazers.jsonl   # one line per stargazer: login, company (public profile)
    python3 stargazer-companies.py count stargazers.jsonl     # distinct companies, and how many developers each has

Needs `gh` logged in. The harvest walks the whole stargazer list (about one GraphQL point per 100 stargazers).
"""
import collections, json, re, subprocess, sys, time

QUERY = ('query($c:String){ repository(owner:"rtk-ai",name:"rtk"){ stargazers(first:100, after:$c){'
         ' pageInfo{hasNextPage endCursor} nodes{ login company } } } }')
SUFFIX = re.compile(r"\b(inc|llc|ltd|limited|corp|corporation|co|gmbh|sa|ag|plc|group|technologies|technology|tech|labs|company|the)\b")
GENERIC = re.compile(r"^(freelance|freelancer|self employed|self|student|none|n a|na|personal|home|me|myself|independent|individual|"
                     r"unemployed|open source|opensource|retired|private|indie|solo|hobbyist|developer|software engineer|engineer|"
                     r"founder|ceo|cto|remote|university|college|school|-|)$")


def harvest(path):
    cursor = None
    with open(path, "w") as out:
        while True:
            args = ["gh", "api", "graphql", "-f", f"query={QUERY}"] + (["-f", f"c={cursor}"] if cursor else [])
            for attempt in range(6):
                run = subprocess.run(args, capture_output=True, text=True)
                if run.returncode == 0:
                    page = json.loads(run.stdout)["data"]["repository"]["stargazers"]
                    break
                time.sleep(5 * (attempt + 1))
            else:
                sys.exit(f"gave up at cursor {cursor}: {run.stderr[-200:]}")
            for node in page["nodes"]:
                if node:
                    out.write(json.dumps({"login": node["login"], "company": node.get("company")}) + "\n")
            if not page["pageInfo"]["hasNextPage"]:
                return
            cursor = page["pageInfo"]["endCursor"]
            time.sleep(0.1)


def normalize(company):
    name = re.sub(r"[^a-z0-9&+ ]+", " ", company.strip().lstrip("@").lower())
    return re.sub(r"\s+", " ", SUFFIX.sub(" ", name)).strip()


def count(path):
    seen = {}
    for line in open(path):
        row = json.loads(line)
        seen.setdefault(row["login"], row.get("company"))
    names = [normalize(c) for c in seen.values() if c]
    tally = collections.Counter(n for n in names if n and not GENERIC.match(n))
    print(f"stargazers={len(seen)} with a company={len(names)}")
    for floor in (1, 2, 3, 5):
        print(f"companies with >= {floor} developers: {sum(1 for v in tally.values() if v >= floor)}")
    for name, n in tally.most_common(60):
        print(f"{n:6d} {name}")


if __name__ == "__main__":
    {"harvest": harvest, "count": count}[sys.argv[1]](sys.argv[2])
