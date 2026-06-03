package analysis

import (
	"context"
	"fmt"
	"log"
	"os/exec"
	"time"
)

const dbtParseTimeout = 120 * time.Second

// runDbtParse executes `<dbtPath> parse --project-dir <projectRoot>` and
// waits up to dbtParseTimeout for it to finish. Any output is written to
// logger.
func runDbtParse(dbtPath, projectRoot string, logger *log.Logger) error {
	ctx, cancel := context.WithTimeout(context.Background(), dbtParseTimeout)
	defer cancel()

	cmd := exec.CommandContext(ctx, dbtPath, "parse", "--project-dir", projectRoot)

	out, err := cmd.CombinedOutput()
	if len(out) > 0 {
		logger.Printf("dbt parse: %s", out)
	}
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return fmt.Errorf("dbt parse timed out after %s", dbtParseTimeout)
		}
		return fmt.Errorf("dbt parse failed: %w", err)
	}
	return nil
}
