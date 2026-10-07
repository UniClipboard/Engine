package main

import "encoding/json"

func marshalJSON(value any) ([]byte, error) { return json.Marshal(value) }

func unmarshalJSON(data []byte, target any) error { return json.Unmarshal(data, target) }
