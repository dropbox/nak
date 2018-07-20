#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate serde_json;
extern crate failure;

use std::collections::{HashMap, HashSet};

use failure::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Unknown(String, Vec<String>),
    SetDirectory(String),
    Edit(String),
}

impl Command {
    pub fn add_args(&mut self, new_args: Vec<String>) {
        match self {
            &mut Command::Unknown(_, ref mut args) => {
                args.extend(new_args)
            }
            &mut Command::SetDirectory(_) |
            &mut Command::Edit(_) => panic!(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub remote_id: usize,
    pub message: RemoteResponseEnvelope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub remote_id: usize,
    pub message: RemoteRequestEnvelope,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ProcessId(usize);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum ExitStatus {
    Success,
    Failure,
}

pub type Condition = Option<ExitStatus>;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ReadPipe(usize);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct WritePipe(usize);

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WriteProcess {
    pub id: ProcessId,
    pub stdin: ReadPipe,
    pub stdout: WritePipe,
    pub stderr: WritePipe,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadProcess {
    pub id: ProcessId,
    pub stdin: WritePipe,
    pub stdout: ReadPipe,
    pub stderr: ReadPipe,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AbstractProcess {
    id: ProcessId,
    stdin: usize,
    stdout: usize,
    stderr: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteRequestEnvelope(RemoteRequest);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteResponseEnvelope(RemoteResponse);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum RemoteRequest {
    BeginCommand {
        block_for: HashMap<ProcessId, Condition>,
        process: AbstractProcess,
        command: Command,
    },
    CancelCommand {
        id: ProcessId,
    },
    BeginRemote {
        id: usize,
        command: Command,
    },
    OpenFile {
        id: usize,
        path: String,
    },
    EndRemote {
        id: usize,
    },
    ListDirectory {
        id: usize,
        path: String,
    },
    FinishEdit {
        id: usize,
        data: Vec<u8>,
    },
    PipeData {
        id: usize,
        data: Vec<u8>,
    },
    PipeRead {
        id: usize,
        count_bytes: usize,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RemoteResponse {
    // RemoteReady {
    //     id: usize,
    //     hostname: String,
    // },
    CommandDone {
        id: ProcessId,
        exit_code: i64,
    },
    DirectoryListing {
        id: usize,
        items: Vec<String>,
    },
    EditRequest {
        command_id: ProcessId,
        edit_id: usize,
        name: String,
        data: Vec<u8>,
    },
    PipeData {
        id: usize,
        data: Vec<u8>,
    },
    PipeRead {
        id: usize,
        count_bytes: usize,
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct RemoteId(usize);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct Handle(usize);

#[derive(Clone)]
struct RemoteState {
    parent: Option<RemoteId>,
}

pub trait Transport {
    fn send(&mut self, msg: &[u8]) -> Result<(), Error>;
}

struct Ids {
    next_id: usize
}

impl Ids {
    fn next_id(&mut self) -> usize {
        let res = self.next_id;
        self.next_id += 1;
        res
    }
}

struct ProcessState {
    parent: RemoteId,
}

pub trait EndpointHandler<T: Transport>: Sized {
    fn command_done(endpoint: &mut Endpoint<T, Self>, id: ProcessId, exit_code: i64) -> Result<(), Error>;
    fn directory_listing(endpoint: &mut Endpoint<T, Self>, id: usize, items: Vec<String>) -> Result<(), Error>;
    fn edit_request(endpoint: &mut Endpoint<T, Self>, edit_id: usize, command_id: ProcessId, name: String, data: Vec<u8>) -> Result<(), Error>;
    fn pipe(endpoint: &mut Endpoint<T, Self>, id: ReadPipe, data: Vec<u8>) -> Result<(), Error>;
    fn pipe_read(endpoint: &mut Endpoint<T, Self>, id: WritePipe, count_bytes: usize) -> Result<(), Error>;
}

pub struct Endpoint<T: Transport, H: EndpointHandler<T>> {
    pub trans: T,
    pub handler: H,
    ids: Ids,
    remotes: HashMap<RemoteId, RemoteState>,
    jobs: HashMap<ProcessId, ProcessState>,
    pipes: HashSet<usize>,
}

fn ser_to_endpoint(remote: RemoteId, message: RemoteRequest) -> Vec<u8> {
    (serde_json::to_string(&Request {
        remote_id: remote.0,
        message: RemoteRequestEnvelope(message),
    }).unwrap() + "\n").into_bytes()
}

fn ser_to_frontend(remote: RemoteId, message: RemoteResponse) -> Vec<u8> {
    (serde_json::to_string(&Response {
        remote_id: remote.0,
        message: RemoteResponseEnvelope(message),
    }).unwrap() + "\n").into_bytes()
}

impl<T: Transport, H: EndpointHandler<T>> Endpoint<T, H> {
    pub fn new(trans: T, handler: H) -> Endpoint<T, H> {
        let mut remotes = HashMap::new();
        remotes.insert(RemoteId(0), RemoteState { parent: None });

        Endpoint {
            trans,
            handler,
            ids: Ids { next_id: 1 },
            remotes,
            jobs: HashMap::new(),
            pipes: HashSet::new(),
        }
    }

    pub fn root(&self) -> RemoteId {
       RemoteId(0)
    }

    pub fn receive(&mut self, message: Response) -> Result<(), Error> {
        match message.message.0 {
            RemoteResponse::CommandDone { id, exit_code } => {
                EndpointHandler::command_done(self, id, exit_code)
            }
            RemoteResponse::DirectoryListing { id, items } => {
                EndpointHandler::directory_listing(self, id, items)
            }
            RemoteResponse::EditRequest { edit_id, command_id, name, data } => {
                EndpointHandler::edit_request(self, edit_id, command_id, name, data)
            }
            RemoteResponse::PipeData { id, data } => {
                assert!(self.pipes.contains(&id));
                EndpointHandler::pipe(self, ReadPipe(id), data)
            }
            RemoteResponse::PipeRead { id, count_bytes } => {
                assert!(self.pipes.contains(&id));
                EndpointHandler::pipe_read(self, WritePipe(id), count_bytes)
            }
        }
    }

    pub fn remote(&mut self, remote: RemoteId, command: Command) -> Result<RemoteId, Error> {
        assert!(self.remotes.contains_key(&remote));

        let id = self.ids.next_id();

        self.trans.send(&ser_to_endpoint(remote, RemoteRequest::BeginRemote {
            id,
            command,
        }))?;

        let old_state = self.remotes.insert(RemoteId(id), RemoteState {
            parent: Some(remote),
        });

        assert!(old_state.is_none());

        Ok(RemoteId(id))
    }

    pub fn command(&mut self, remote: RemoteId, command: Command, block_for: HashMap<ProcessId, Condition>, redirect: Option<Handle>) -> Result<ReadProcess, Error> {
        assert!(self.remotes.contains_key(&remote));

        let id = ProcessId(self.ids.next_id());
        let stdin_pipe = self.ids.next_id();
        let stdout_pipe = match redirect {
            Some(h) => {
                assert!(self.pipes.remove(&h.0));
                h.0
            }
            None => {
                let id = self.ids.next_id();
                self.pipes.insert(id);
                id
            }
        };
        let stderr_pipe = self.ids.next_id();

        self.pipes.insert(stderr_pipe);

        let process = AbstractProcess {
            id,
            stdin: stdin_pipe,
            stdout: stdout_pipe,
            stderr: stderr_pipe,
        };

        self.trans.send(&ser_to_endpoint(remote, RemoteRequest::BeginCommand {
            block_for,
            process,
            command,
        }))?;

        self.jobs.insert(id, ProcessState { parent: remote });

        Ok(ReadProcess {
            id,
            stdin: WritePipe(stdin_pipe),
            stdout: ReadPipe(stdout_pipe),
            stderr: ReadPipe(stderr_pipe),
        })
    }

    pub fn open_file(&mut self, remote: RemoteId, path: String) -> Result<Handle, Error> {
        assert!(self.remotes.contains_key(&remote));

        let id = self.ids.next_id();

        self.pipes.insert(id);

        self.trans.send(&ser_to_endpoint(remote, RemoteRequest::OpenFile {
            id,
            path,
        }))?;

        Ok(Handle(id))
    }

    pub fn close_remote(&mut self, remote: RemoteId) -> Result<(), Error> {
        let state = self.remotes.remove(&remote).expect("remote not connected");

        // TODO: close jobs?

        self.trans.send(&ser_to_endpoint(state.parent.expect("closing root remote"), RemoteRequest::EndRemote {
            id: remote.0,
        }))?;

        Ok(())
    }

    pub fn close_process(&mut self, id: ProcessId) -> Result<(), Error> {
        let process_state = self.jobs.remove(&id).expect("process not running");

        assert!(self.remotes.contains_key(&process_state.parent));

        self.trans.send(&ser_to_endpoint(process_state.parent, RemoteRequest::CancelCommand {
            id,
        }))?;

        Ok(())
    }

    pub fn finish_edit(&mut self, command_id: ProcessId, edit_id: usize, data: Vec<u8>) -> Result<(), Error> {
        let process_state = self.jobs.get(&command_id).expect("process not running");

        assert!(self.remotes.contains_key(&process_state.parent));

        self.trans.send(&ser_to_endpoint(process_state.parent, RemoteRequest::FinishEdit {
            id: edit_id,
            data,
        }))?;

        Ok(())
    }
}

pub trait BackendHandler {
    fn begin_command(&mut self, block_for: HashMap<ProcessId, Condition>, process: WriteProcess, command: Command) -> Result<(), Error>;
    fn cancel_command(&mut self, id: ProcessId) -> Result<(), Error>;
    fn begin_remote(&mut self, id: usize, command: Command) -> Result<(), Error>;
    fn open_file(&mut self, id: WritePipe, path: String) -> Result<(), Error>;
    fn end_remote(&mut self, id: usize) -> Result<(), Error>;
    fn list_directory(&mut self, id: usize, path: String) -> Result<(), Error>;
    fn finish_edit(&mut self, id: usize, data: Vec<u8>) -> Result<(), Error>;
    fn pipe_data(&mut self, id: usize, data: Vec<u8>) -> Result<(), Error>;
    fn pipe_read(&mut self, id: usize, count_bytes: usize) -> Result<(), Error>;
}

#[derive(Default)]
pub struct Backend<T: Transport> {
    pub trans: T,
}

impl Request {
    pub fn route<H: BackendHandler>(self, handler: &mut H) -> Result<(), Error> {
        match self.message.0 {
            RemoteRequest::BeginCommand { block_for, process, command, } => {
                let process = WriteProcess {
                    id: process.id,
                    stdin: ReadPipe(process.stdin),
                    stdout: WritePipe(process.stdout),
                    stderr: WritePipe(process.stderr),
                };
                handler.begin_command(block_for, process, command)
            }
            RemoteRequest::CancelCommand { id, } => {
                handler.cancel_command(id)
            }
            RemoteRequest::BeginRemote { id, command, } => {
                handler.begin_remote(id, command)
            }
            RemoteRequest::OpenFile { id, path, } => {
                handler.open_file(WritePipe(id), path)
            }
            RemoteRequest::EndRemote { id, } => {
                handler.end_remote(id)
            }
            RemoteRequest::ListDirectory { id, path, } => {
                handler.list_directory(id, path)
            }
            RemoteRequest::FinishEdit { id, data, } => {
                handler.finish_edit(id, data)
            }
            RemoteRequest::PipeData { id, data, } => {
                handler.pipe_data(id, data)
            }
            RemoteRequest::PipeRead { id, count_bytes, } => {
                handler.pipe_read(id, count_bytes)
            }
        }
    }
}

impl<T: Transport> Backend<T> {
    pub fn new(trans: T) -> Backend<T> {
        Backend {
            trans,
        }
    }

    pub fn pipe(&mut self, id: WritePipe, data: Vec<u8>) -> Result<(), Error> {
        self.trans.send(&ser_to_frontend(RemoteId(0), RemoteResponse::PipeData {
            id: id.0,
            data,
        }))?;

        Ok(())
    }

    pub fn command_done(&mut self, id: ProcessId, exit_code: i64) -> Result<(), Error> {
        self.trans.send(&ser_to_frontend(RemoteId(0), RemoteResponse::CommandDone {
            id,
            exit_code,
        }))?;

        Ok(())
    }

    pub fn directory_listing(&mut self, id: usize, items: Vec<String>) -> Result<(), Error> {
        self.trans.send(&ser_to_frontend(RemoteId(0), RemoteResponse::DirectoryListing {
            id,
            items,
        }))?;

        Ok(())
    }

    pub fn edit_request(&mut self, command_id: ProcessId, edit_id: usize, name: String, data: Vec<u8>) -> Result<(), Error> {
        self.trans.send(&ser_to_frontend(RemoteId(0), RemoteResponse::EditRequest {
            command_id,
            edit_id,
            name,
            data,
        }))?;

        Ok(())
    }
}
