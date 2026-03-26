-- 70% write (POST), 30% read (GET)
wrk.body   = '{"message": "Hello World!"}'
wrk.headers["Content-Type"] = "application/json"

request = function()
   local r = math.random()
   if r < 0.7 then
      -- 70% POST (write)
      wrk.method = "POST"
      return wrk.format("POST", nil, wrk.headers, wrk.body)
   else
      -- 30% GET (read)
      wrk.method = "GET"
      return wrk.format("GET", nil, wrk.headers, nil)
   end
end