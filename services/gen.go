package services

//go:generate mkdir -p pb
//go:generate protoc --go_out=. --go-grpc_out=. service.proto
