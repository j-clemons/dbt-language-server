package services

import (
	"context"
	"log"
    "path/filepath"
    "os"
	"os/exec"
	"time"

	pb "github.com/j-clemons/dbt-language-server/services/pb"

	"google.golang.org/grpc"
)

func StartPythonServer() (*exec.Cmd, error) {
	exePath, err := os.Executable()
	if err != nil {
        return nil, err
	}
	exeDir := filepath.Dir(exePath)

	scriptPath := filepath.Join(exeDir, "services/py/server.py")

	cmd := exec.Command("python", scriptPath)
	cmd.Stdout = log.Writer()
	cmd.Stderr = log.Writer()

	err = cmd.Start()
	if err != nil {
        return nil, err
	}

	return cmd, nil
}

func PythonServer() (pb.MyServiceClient, error) {
	conn, err := grpc.Dial("localhost:50051", grpc.WithInsecure())
	if err != nil {
        return nil, err
	}

	return pb.NewMyServiceClient(conn), nil
}

func Lint(client pb.MyServiceClient, message string, cfgPath string) []*pb.LintResultItem {
    ctx, cancel := context.WithTimeout(context.Background(), time.Second)
    defer cancel()

	req := &pb.LintRequest{
        FileString:     message,
        SqfluffCfgPath: cfgPath,
    }
	res, err := client.Lint(ctx, req)
	if err != nil {
		log.Fatalf("Error calling Lint: %v", err)
	}

    return res.Items
}
