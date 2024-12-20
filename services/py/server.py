import grpc
from concurrent import futures
import service_pb2
import service_pb2_grpc


class MyServiceServicer(service_pb2_grpc.MyServiceServicer):
    def SayHello(self, request, context):
        print("Received message: %s" % request.message)
        reply = f"Hello, {request.message}!"
        return service_pb2.Response(reply=reply)


def serve():
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    service_pb2_grpc.add_MyServiceServicer_to_server(MyServiceServicer(), server)
    server.add_insecure_port("[::]:50051")
    server.start()
    server.wait_for_termination()


if __name__ == "__main__":
    serve()
