# expect: invalid call to 'm'
# A method's own origin binder no argument names fails to infer (pinned
# Mojo: "invalid call to 'm': failed to infer parameter
# 'origin_of(_mlir_origin)'").
struct S:
    def __init__(out self):
        pass

    def m[o: ImmOrigin](self) -> Int:
        return 3


def main():
    print(S().m())
