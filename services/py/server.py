import signal
import sys
import grpc
from concurrent import futures
import service_pb2
import service_pb2_grpc
import sqlfluff
from sqlfluff.core import FluffConfig


class MyServiceServicer(service_pb2_grpc.MyServiceServicer):
    def Lint(self, request, context):
        print("Received file")
        config = FluffConfig.from_path(
            path=request.sqfluff_cfg_path,
            overrides={"templater": "jinja"}
        )
        config.set_value(['ignore'], 'templating')
        lint_results = sqlfluff.lint(request.file_string, config=config)
        cleaned_lint_results = []
        for lr in lint_results:
            cleaned_lint_results.append(clean_lint_result(lr))
        return service_pb2.LintResult(items=cleaned_lint_results)


def clean_lint_result(lr: dict) -> dict:
    cleaned_lr = {
        "code": lr.get("code"),
        "description": lr.get("description"),
        "name": lr.get("name"),
        "warning": lr.get("warning"),
        "start_line_no": lr.get("start_line_no"),
        "start_line_pos": lr.get("start_line_pos"),
        "end_line_no": lr.get("end_line_no"),
        "end_line_pos": lr.get("end_line_pos"),
    }

    return cleaned_lr


def signal_handler(sig, frame):
    print("Received shutdown signal")
    if hasattr(signal_handler, 'server'):
        print("Stopping gRPC server gracefully...")
        signal_handler.server.stop(grace=5)  # 5 seconds grace period
    sys.exit(0)


def serve():
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    service_pb2_grpc.add_MyServiceServicer_to_server(MyServiceServicer(), server)
    server.add_insecure_port("[::]:50051")

    # Store server instance in signal_handler for access during shutdown
    signal_handler.server = server

    # Set up signal handlers
    signal.signal(signal.SIGTERM, signal_handler)
    signal.signal(signal.SIGINT, signal_handler)

    server.start()
    try:
        server.wait_for_termination()
    except KeyboardInterrupt:
        signal_handler(signal.SIGINT, None)


if __name__ == "__main__":
    serve()
