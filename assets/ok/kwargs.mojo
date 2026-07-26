def total(**kwargs: Int) -> Int:
    return len(kwargs)

def main():
    print(total(first=1, second=2, third=3))
