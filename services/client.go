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

func createVirtualEnv(venvPath string, requirementsFile string) {
	if _, err := os.Stat(venvPath); os.IsNotExist(err) {
		// Virtual environment does not exist, create and install dependencies
		cmd := exec.Command("python", "-m", "venv", venvPath)
		cmd.Stdout = log.Writer()
		cmd.Stderr = log.Writer()
		if err := cmd.Run(); err != nil {
			log.Fatalf("Failed to create virtual environment: %v", err)
		}
        installDependencies(venvPath, requirementsFile)
	}
}

func installDependencies(venvPath, requirementsFile string) {
	pipPath := filepath.Join(venvPath, "bin", "pip")
	cmd := exec.Command(pipPath, "install", "-r", requirementsFile)
	cmd.Stdout = log.Writer()
	cmd.Stderr = log.Writer()
	if err := cmd.Run(); err != nil {
		log.Fatalf("Failed to install dependencies: %v", err)
	}
}

func StartPythonServer() (*exec.Cmd, error) {
	exePath, err := os.Executable()
	if err != nil {
        return nil, err
	}
	exeDir := filepath.Dir(exePath)

    venvPath := filepath.Join(exeDir, "services/py/venv")
    requirementsFile := filepath.Join(exeDir, "services/py/requirements.txt")
    createVirtualEnv(venvPath, requirementsFile)

	scriptPath := filepath.Join(exeDir, "services/py/server.py")
    venvPythonPath := filepath.Join(venvPath, "bin", "python")
	cmd := exec.Command(venvPythonPath, scriptPath)
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
