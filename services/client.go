package services

import (
	"context"
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

func Lint(client pb.MyServiceClient, message string) []*pb.LintResultItem {
    ctx, cancel := context.WithTimeout(context.Background(), time.Second)
    defer cancel()

	req := &pb.FileString{FileString: message}
	res, err := client.Lint(ctx, req)
	if err != nil {
		log.Fatalf("Error calling Lint: %v", err)
	}

    return res.Items
}
