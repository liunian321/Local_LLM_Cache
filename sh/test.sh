curl -v http://127.0.0.1:1234/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "maziyarpanahi/gemma-3-4b-it",
    "messages": [
      { "role": "system", "content": "Always answer in rhymes. Today is Thursday" },
      { "role": "user", "content": "Hello, how are you?" }
    ],
    "temperature": 0.7,
    "max_tokens": -1,
    "stream": false
}'