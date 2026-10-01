def pick(var **kwargs: Int) raises -> Int:
    return kwargs["b"]

def main() raises:
    print(pick(a=1, b=2))
