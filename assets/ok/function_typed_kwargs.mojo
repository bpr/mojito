def count(var **options: Int) -> Int:
    return len(options)

def main():
    var callback: def(var **options: Int) thin -> Int = count
    print(callback(first=1, second=2))
