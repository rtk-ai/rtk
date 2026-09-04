---
inclusion: always
---
# RTK — Rust Token Killer (Kiro)

Prefixe comandos shell com `rtk` para reduzir 60-90% do consumo de tokens.
Em cadeias com `&&`, `||`, `;`, prefixe cada comando.

## Exemplos

```bash
rtk git status
rtk cargo test
rtk ls src/
rtk grep "padrão" src/
rtk docker ps
rtk gh pr list
```

## Meta comandos

```bash
rtk gain            # economia de tokens
rtk discover        # oportunidades perdidas de rtk
rtk proxy <cmd>     # executa sem filtrar (debug)
```

## Por quê

O RTK filtra e comprime a saída dos comandos antes de chegar ao contexto do LLM.
Use sempre `rtk <cmd>` no lugar do comando bruto.
