"""Explicit process ownership for the concurrency drill's Windows descendants."""
import ctypes
import os


class WindowsScope:
    """A private job inherited from the drill, excluding the drill itself from cleanup."""

    def __init__(self):
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p]
        kernel.CreateJobObjectW.restype = ctypes.c_void_p
        kernel.AssignProcessToJobObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
        kernel.AssignProcessToJobObject.restype = ctypes.c_int
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.QueryInformationJobObject.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p,
                                                     ctypes.c_ulong, ctypes.c_void_p]
        kernel.QueryInformationJobObject.restype = ctypes.c_int
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        self.kernel = kernel
        self.handle = kernel.CreateJobObjectW(None, None)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel.AssignProcessToJobObject(self.handle, kernel.GetCurrentProcess()):
            error = ctypes.WinError(ctypes.get_last_error())
            kernel.CloseHandle(self.handle)
            self.handle = None
            raise error

    def pids(self):
        capacity = 32
        while True:
            buffer = ctypes.create_string_buffer(8 + capacity * ctypes.sizeof(ctypes.c_size_t))
            if self.kernel.QueryInformationJobObject(self.handle, 3, buffer, len(buffer), None):
                count = ctypes.c_ulong.from_buffer(buffer, 4).value
                ids = (ctypes.c_size_t * count).from_buffer(buffer, 8)
                return {pid for pid in ids if pid != os.getpid()}
            error = ctypes.get_last_error()
            if error != 234:
                raise ctypes.WinError(error)
            capacity *= 2

    def close(self):
        if self.handle:
            if not self.kernel.CloseHandle(self.handle):
                raise ctypes.WinError(ctypes.get_last_error())
            self.handle = None
