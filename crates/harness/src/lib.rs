use std::future::Future;
use std::process::{Child, Command};
use std::time::Duration;

/// A single step of a test
pub enum TestStep {
    /// A step that executes over services, such as starting or stopping a service
    Service(Box<dyn ServiceStepExecutor<StepError = String>>),
    /// A step that executes an async function
    AsyncFn(Box<dyn FnOnce() -> Box<dyn Future<Output = Result<(), String>>>>),
}

/// A harness for running tests with services
/// It manages the lifecycle of services and executes test steps
pub struct Harness {
    pub test_name: String,
    pub root_dir: String,
    pub services: Vec<Box<dyn Service<ServiceError = String>>>,
    pub steps: Vec<TestStep>,
}

impl Harness {
    pub fn new(test_name: &str, root_dir: &str) -> Self {
        Self {
            test_name: test_name.to_string(),
            root_dir: root_dir.to_string(),
            services: Vec::new(),
            steps: Vec::new(),
        }
    }

    pub fn add_service(&mut self, service: Box<dyn Service<ServiceError = String>>) {
        self.services.push(service);
    }

    pub fn add_step(&mut self, step: TestStep) { self.steps.push(step); }

    pub fn execute(mut self) -> Result<(), String> {
        for step in self.steps {
            match step {
                TestStep::Service(step_executor) => {
                    step_executor.execute(self.services.as_mut_slice())?;
                }
                TestStep::AsyncFn(future) => {
                    let _ = tokio::runtime::Runtime::new()
                        .map_err(|e| format!("Failed to create runtime: {}", e))?
                        .block_on(Box::into_pin(future()));
                }
            }
        }
        Ok(())
    }
}

pub trait ServiceStepExecutor {
    type StepError;
    fn execute(
        &self,
        services: &mut [Box<dyn Service<ServiceError = String>>],
    ) -> Result<(), Self::StepError>;
}

pub struct SubProcessServiceStarter {
    pub name: String,
    pub description: String,
    pub service_idx: usize,
    pub wait_after: Option<Duration>,
}

impl ServiceStepExecutor for SubProcessServiceStarter {
    type StepError = String;

    fn execute(
        &self,
        services: &mut [Box<dyn Service<ServiceError = String>>],
    ) -> Result<(), Self::StepError> {
        // Implementation of the step execution logic
        assert!(services.len() == 1, "Expected exactly one service");
        let service = &mut services[self.service_idx];
        if service.is_running() {
            return Err(format!("Service '{}' is already running", self.name));
        }
        service
            .start()
            .map_err(|e| format!("Failed to start service '{}': {}", self.name, e))?;
        if let Some(wait_duration) = self.wait_after {
            std::thread::sleep(wait_duration);
        }
        Ok(())
    }
}

pub struct SubProcessServiceStopper {
    pub name: String,
    pub description: String,
    pub service_idx: usize,
    pub wait_after: Option<Duration>,
}

impl ServiceStepExecutor for SubProcessServiceStopper {
    type StepError = String;

    fn execute(
        &self,
        services: &mut [Box<dyn Service<ServiceError = String>>],
    ) -> Result<(), Self::StepError> {
        // Implementation of the step execution logic
        assert!(services.len() == 1, "Expected exactly one service");
        let service = &mut services[0];
        if !service.is_running() {
            return Err(format!("Service '{}' is not running", self.name));
        }
        service
            .stop()
            .map_err(|e| format!("Failed to stop service '{}': {}", self.name, e))?;
        if let Some(wait_duration) = self.wait_after {  
            std::thread::sleep(wait_duration);
        }
        Ok(())
    }
}

pub trait Service {
    type ServiceError;
    fn start(&mut self) -> Result<(), Self::ServiceError>;
    fn is_running(&self) -> bool;
    fn stop(&mut self) -> Result<(), Self::ServiceError>;
}

pub struct SubProcessService {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub child: Option<Child>,
}

impl Service for SubProcessService {
    type ServiceError = String;

    fn start(&mut self) -> Result<(), String> {
        if self.is_running() {
            return Err(format!("Subprocess '{}' is already running", self.name));
        }
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);

        match cmd.spawn() {
            Ok(child) => {
                self.child = Some(child);
                Ok(())
            }
            Err(e) => Err(format!("Failed to start subprocess '{}': {}", self.name, e)),
        }
    }

    fn is_running(&self) -> bool { self.child.is_some() }

    fn stop(&mut self) -> Result<(), String> {
        if !self.is_running() {
            return Err(format!("Subprocess '{}' is not running", self.name));
        }
        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Failed to stop subprocess '{}': {}", self.name, e)),
            }
        } else {
            Err(format!("Subprocess '{}' is not running", self.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_callapi_stop_python_servef() {
        let mut harness = Harness::new("PythonServerTester", ".");

        harness.add_service(Box::new(SubProcessService {
            name: "Python_HTTP_Service".to_string(),
            command: "python3".to_string(),
            args: vec![
                "-m".to_string(),
                "http.server".to_string(),
                "8081".to_string(),
            ],
            child: None,
        }));

        harness.add_step(TestStep::Service(Box::new(SubProcessServiceStarter {
            name: "Python_HTTP_Service".to_string(),
            description: "Starts the Python HTTP server".to_string(),
            service_idx: 0,
            wait_after: Some(Duration::from_secs(2)),
        })));

        harness.add_step(TestStep::AsyncFn(Box::new(|| {
            Box::new(async {
                let response = reqwest::get("http://localhost:8081").await;

                match response {
                    Ok(resp) =>
                        if resp.status() == 200 {
                            Ok(())
                        } else {
                            Err(format!("API call failed: Status code {}", resp.status()))
                        },
                    Err(e) => Err(format!("Failed to make API call: {}", e)),
                }
            })
        })));

        harness.add_step(TestStep::Service(Box::new(SubProcessServiceStopper {
            name: "Python_HTTP_Service".to_string(),
            description: "Stops the Python HTTP server".to_string(),
            service_idx: 0,
            wait_after: None,
        })));

        harness.execute().expect("Failed to execute test steps");
    }
}
