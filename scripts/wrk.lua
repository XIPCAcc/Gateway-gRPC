wrk.method = "POST"
wrk.body   = '{"message": "Hello World!"}'
wrk.headers["Content-Type"] = "application/json"

request = function()
   return wrk.format(nil, nil, nil)
end