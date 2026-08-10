def enabled[T: AnyType](value: T) -> Int where (True, "enabled only"):
    return 1

def main():
    print(enabled(42))
