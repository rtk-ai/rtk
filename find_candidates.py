import json, subprocess
result = subprocess.run(['gh','issue','list','--repo','rtk-ai/rtk','--state','open','--limit','200','--json','number,title,assignees,labels,url,closedByPullRequestsReferences'], capture_output=True, text=True)
issues=json.loads(result.stdout)
candidates=[]
for i in issues:
    if i['assignees']:
        continue
    # skip if closedByPullRequestsReferences non-empty
    if i.get('closedByPullRequestsReferences'):
        continue
    # also check labels good first issue maybe prioritized
    has_gfi= any(l['name']=='good first issue' for l in i['labels'])
    candidates.append((has_gfi,i))
# sort by has_gfi then number
candidates.sort(key=lambda x: (not x[0], x[1]['number']))
for has_gfi,i in candidates[:100]:
    print(f"{i['number']} | {i['title']} | gfi={has_gfi} | labels={[l['name'] for l in i['labels']]}")
