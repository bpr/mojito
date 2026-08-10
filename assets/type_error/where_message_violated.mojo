# expect: constraint failed: this specialization is unavailable
def disabled[T: AnyType](value: T) -> Int where (
    False, "this specialization is unavailable"
):
    return 1

def main():
    print(disabled(42))
