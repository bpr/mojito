def fail() raises:
    raise Error("boom")

def relay() raises:
    try:
        fail()
    except e:
        raise e

def main():
    try:
        relay()
    except e:
        print("caught", e)
