package services

import (
	"context"
	"fmt"
	"log"
	"time"

	pb "github.com/j-clemons/dbt-language-server/services/pb"

	"google.golang.org/grpc"
)

func PythonServer() (pb.MyServiceClient, error) {
	conn, err := grpc.Dial("localhost:50051", grpc.WithInsecure())
	if err != nil {
        return nil, err
	}

	return pb.NewMyServiceClient(conn), nil
}

func SayHello(client pb.MyServiceClient, message string) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()

	req := &pb.Request{Message: message}
	res, err := client.SayHello(ctx, req)
	if err != nil {
		log.Fatalf("Error calling SayHello: %v", err)
	}
	fmt.Printf("SayHello Response: %s\n", res.Reply)
}
