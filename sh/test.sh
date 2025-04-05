curl http://192.168.1.11:1234/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "maziyarpanahi/gemma-3-12b-it",
    "messages": [
      { "role": "system", "content": "Always answer in rhymes. Today is Thursday" },
      { "role": "user", "content": "What day is it today?" }
    ],
    "temperature": 0.7,
    "max_tokens": -1,
    "stream": false
}'