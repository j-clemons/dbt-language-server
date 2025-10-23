package util

import (
	"fmt"
	"os/exec"
)

func ValidateFusion(fusion string) (bool, error) {
	cmd := exec.Command(fusion, "--version")
	output, err := cmd.Output()
	if err != nil {
		return false, err
	} else if string(output)[0:len("dbt-fusion")] == "dbt-fusion" {
		return true, nil
	}

	return false, fmt.Errorf("Invalid dbt-fusion alias: %s", string(output))
}
