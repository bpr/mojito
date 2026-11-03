@fieldwise_init
struct RaisingCallable(def() raises -> Int):
    def __call__(self) raises -> Int:
        raise Error("expected")


def main():
    try:
        print(RaisingCallable()())
    except:
        print(42)
