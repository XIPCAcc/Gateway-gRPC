local matrix_size = os.getenv("MATRIX_SIZE") or "16"
wrk.body = '{"matrix_size":' .. matrix_size .. '}'
wrk.headers["Content-Type"] = "application/json"

request = function()
   return wrk.format("POST", nil, wrk.headers, wrk.body)
end
