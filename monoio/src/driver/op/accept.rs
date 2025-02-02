#[cfg(all(unix, any(feature = "legacy", feature = "poll-io")))]
use std::os::unix::prelude::AsRawFd;
use std::{
    io,
    mem::{size_of, MaybeUninit},
};

#[cfg(all(target_os = "linux", feature = "iouring"))]
use io_uring::{opcode, types};
#[cfg(windows)]
use {
    std::os::windows::prelude::AsRawSocket,
    windows_sys::Win32::Networking::WinSock::{
        accept, socklen_t, INVALID_SOCKET, SOCKADDR_STORAGE,
    },
};

use super::{super::shared_fd::SharedFd, Op, OpAble};
#[cfg(any(feature = "legacy", feature = "poll-io"))]
use super::{driver::ready::Direction, MaybeFd};

/// Accept
pub(crate) struct Accept {
    pub(crate) fd: SharedFd,
    #[cfg(unix)]
    pub(crate) addr: Box<(MaybeUninit<libc::sockaddr_storage>, libc::socklen_t)>,
    #[cfg(windows)]
    pub(crate) addr: Box<(MaybeUninit<SOCKADDR_STORAGE>, socklen_t)>,
}

impl Op<Accept> {
    /// Accept a connection
    pub(crate) fn accept(fd: &SharedFd) -> io::Result<Self> {
        #[cfg(unix)]
        let addr = Box::new((
            MaybeUninit::uninit(),
            size_of::<libc::sockaddr_storage>() as libc::socklen_t,
        ));

        #[cfg(windows)]
        let addr = Box::new((
            MaybeUninit::uninit(),
            size_of::<SOCKADDR_STORAGE>() as socklen_t,
        ));

        Op::submit_with(Accept {
            fd: fd.clone(),
            addr,
        })
    }
}

impl OpAble for Accept {
    #[cfg(all(target_os = "linux", feature = "iouring"))]
    const RET_IS_FD: bool = true;

    #[cfg(all(target_os = "linux", feature = "iouring"))]
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::Accept::new(
            types::Fd(self.fd.raw_fd()),
            self.addr.0.as_mut_ptr() as *mut _,
            &mut self.addr.1,
        )
        .build()
    }

    #[cfg(any(feature = "legacy", feature = "poll-io"))]
    #[inline]
    fn legacy_interest(&self) -> Option<(Direction, usize)> {
        self.fd.registered_index().map(|idx| (Direction::Read, idx))
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), windows))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_socket();
        let addr = self.addr.0.as_mut_ptr() as *mut _;
        let len = &mut self.addr.1;

        crate::syscall!(accept@FD(fd as _, addr, len), PartialEq::eq, INVALID_SOCKET)
    }

    #[cfg(all(any(feature = "legacy", feature = "poll-io"), unix))]
    fn legacy_call(&mut self) -> io::Result<MaybeFd> {
        let fd = self.fd.as_raw_fd();
        let addr = self.addr.0.as_mut_ptr() as *mut _;
        let len = &mut self.addr.1;
        // Here I use copied some code from mio because I don't want the conversion.

        // On platforms that support it we can use `accept4(2)` to set `NONBLOCK`
        // and `CLOEXEC` in the call to accept the connection.
        #[cfg(any(
            // Android x86's seccomp profile forbids calls to `accept4(2)`
            // See https://github.com/tokio-rs/mio/issues/1445 for details
            all(
                not(target_arch="x86"),
                target_os = "android"
            ),
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "illumos",
            target_os = "linux",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        return {
            let flag = libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK;
            crate::syscall!(accept4@FD(fd, addr, len, flag))
        };

        // But not all platforms have the `accept4(2)` call. Luckily BSD (derived)
        // OSes inherit the non-blocking flag from the listener, so we just have to
        // set `CLOEXEC`.
        #[cfg(any(
            all(target_arch = "x86", target_os = "android"),
            target_os = "ios",
            target_os = "macos",
            target_os = "redox"
        ))]
        return {
            let stream_fd = crate::syscall!(accept@FD(fd, addr, len))?;
            let fd = stream_fd.fd() as libc::c_int;
            crate::syscall!(fcntl@RAW(fd, libc::F_SETFD, libc::FD_CLOEXEC))?;
            crate::syscall!(fcntl@RAW(fd, libc::F_SETFL, libc::O_NONBLOCK))?;
            Ok(stream_fd)
        };
    }
}
