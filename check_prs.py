import json, subprocess, sys
result = subprocess.run(['gh','pr','list','--repo','rtk-ai/rtk','--state','open','--json','number,title,body,url'], capture_output=True, text=True)
data=json.loads(result.stdout)
nums=[4300,4298,4289,4268,4249,4237]
for p in data:
    txt=(p['title']+' '+(p.get('body') or '')).lower()
    for n in nums:
        if f'#{n}' in txt or f' {n} ' in txt:
            print(p['number'],p['title'],n)
print('done')
