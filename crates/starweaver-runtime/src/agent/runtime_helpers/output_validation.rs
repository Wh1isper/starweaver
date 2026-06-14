//! Output function and final output validation helpers.

use starweaver_context::AgentContext;

use crate::{
    agent::Agent,
    capability::CapabilityError,
    output::{parse_output, OutputFunctionContext, OutputValue},
    run::AgentRunState,
};

impl Agent {
    pub(in crate::agent) async fn try_call_output_function(
        &self,
        state: &AgentRunState,
        calls: &[starweaver_model::ToolCallPart],
    ) -> Result<Option<(String, Option<serde_json::Value>)>, CapabilityError> {
        let Some(call) = calls.iter().find(|call| {
            self.output_functions
                .iter()
                .any(|function| function.definition().name == call.name)
        }) else {
            return Ok(None);
        };
        let function = self
            .output_functions
            .iter()
            .find(|function| function.definition().name == call.name)
            .ok_or_else(|| {
                CapabilityError::Failed(format!("missing output function {}", call.name))
            })?;
        match function
            .call(
                OutputFunctionContext {
                    state: state.clone(),
                },
                call.arguments.execution_value(),
            )
            .await
            .map_err(Self::output_validation_error)
        {
            Ok(output) => Ok(Some((output.as_text(), output.as_json().cloned()))),
            Err(error) => Err(error),
        }
    }

    pub(in crate::agent) async fn validate_final_output(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        self.call_before_output_validation(state, context, output)
            .await?;
        let parsed = parse_output(output, self.output_schema.as_ref())
            .map_err(Self::output_validation_error)?;
        state.structured_output = parsed.as_json().cloned();
        self.call_output_validators(state, &parsed).await?;
        self.call_validate_output(state, context, output).await?;
        self.call_after_output_validation(state, context, output)
            .await
    }

    pub(in crate::agent) async fn call_before_output_validation(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.ordered_capabilities_for_validation()? {
            capability
                .before_output_validation_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_output_validators(
        &self,
        state: &mut AgentRunState,
        output: &OutputValue,
    ) -> Result<(), CapabilityError> {
        for validator in &self.output_validators {
            validator
                .validate(state, output)
                .await
                .map_err(Self::output_validation_error)?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_validate_output(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.ordered_capabilities_for_validation()? {
            capability
                .validate_output_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }

    pub(in crate::agent) async fn call_after_output_validation(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        output: &str,
    ) -> Result<(), CapabilityError> {
        for capability in &self.ordered_capabilities_for_validation()? {
            capability
                .after_output_validation_with_context(state, context, output)
                .await?;
        }
        Ok(())
    }
}
