import grpc
from concurrent import futures
import service_pb2
import service_pb2_grpc
import sqlfluff


class MyServiceServicer(service_pb2_grpc.MyServiceServicer):
    def Lint(self, request, context):
        print("Received file")
        lint_results = sqlfluff.lint(request.file_string)
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


def serve():
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    service_pb2_grpc.add_MyServiceServicer_to_server(MyServiceServicer(), server)
    server.add_insecure_port("[::]:50051")
    server.start()
    server.wait_for_termination()


if __name__ == "__main__":
    serve()
