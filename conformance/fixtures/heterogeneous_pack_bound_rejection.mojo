def count[*ArgTypes: Intable](*args: *ArgTypes) -> Int:
    return len(args)

def main():
    print(count(1, "two", True))
